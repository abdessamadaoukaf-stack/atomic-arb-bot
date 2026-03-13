import os
import json
import warnings
import requests
from datetime import datetime, timezone
from dataclasses import dataclass
import numpy as np
import pandas as pd
import matplotlib
matplotlib.use("Agg")
import mplfinance as mpf
from sklearn.linear_model import HuberRegressor
from sklearn.preprocessing import StandardScaler
from sklearn.pipeline import Pipeline

warnings.filterwarnings("ignore")

@dataclass
class InstrumentConfig:
    name: str; point_value: float; tick_size: float; tick_value: float; commission_rt: float; slippage_ticks: int
    atr_stop_mult: float = 1.5; atr_tp_mult: float = 2.25; account_size: float = 100_000.0
    combine_profit_target_pct: float = 0.06; combine_max_dd_pct: float = 0.05; risk_per_trade_pct: float = 0.0035
    @property
    def slippage_points(self) -> float: return self.slippage_ticks * self.tick_size
    @property
    def point_friction(self) -> float: return (self.slippage_ticks * self.tick_value * 2 + self.commission_rt) / self.point_value

ES_CONFIG = InstrumentConfig("ES", 50.0, 0.25, 12.50, 4.0, 2)
MNQ_CONFIG = InstrumentConfig("MNQ", 2.0, 0.25, 0.50, 1.0, 2)

def build_ml_features(df: pd.DataFrame, n_lags=5) -> tuple:
    d = df.copy(); d.columns = [c.lower() for c in d.columns]
    d["log_ret"] = np.log(d["close"] / d["close"].shift(1)); d["target"] = d["log_ret"].shift(-1)
    for i in range(1, n_lags + 1): d[f"lag_{i}"] = d["log_ret"].shift(i)
    d["vol"] = d["log_ret"].rolling(10).std()
    d["zscore"] = d["log_ret"] / d["vol"].replace(0, np.nan)
    d["atr"] = np.log(d["high"] / d["low"]).rolling(14).mean()
    f_cols = [f"lag_{i}" for i in range(1, n_lags+1)] + ["vol", "zscore", "atr"]
    o_cols = list(dict.fromkeys(f_cols + ["target", "open", "high", "low", "close", "volume", "atr"]))
    return d[[c for c in o_cols if c in d.columns]].dropna(), f_cols

def get_risk(eq, pk, base, max_dd):
    if pk <= 0: return base
    dd = max_dd - ((pk - eq) / pk)
    return base if dd >= 0.015 else (base * 0.5) + (base * 0.5) * (dd / 0.015) if dd > 0 else base * 0.5

def train_and_predict(f_df, f_cols, eps, cm):
    X, y = f_df[f_cols].values, f_df["target"].values
    pipe = Pipeline([("s", StandardScaler()), ("m", HuberRegressor(epsilon=eps, alpha=0.01, max_iter=300))])
    idx = int(len(X)*0.8)
    pipe.fit(X[:idx], y[:idx])
    pred = pipe.predict(X[idx:])
    ps = pd.Series(pred); thr = cm * ps.rolling(20, min_periods=5).std().fillna(ps.abs().mean() * cm)
    return float(pred[-1]), float(thr.iloc[-1])

def process_trade(df, action, contracts, price, sl_pts, tp_pts, conf, thr, cfg):
    L_FILE = "ledger.json"
    led = json.load(open(L_FILE)) if os.path.exists(L_FILE) else {"equity": 100000.0, "peak_equity": 100000.0, "open_trade": None}
    d_url, j_url = os.environ.get("DISCORD_WEBHOOK", ""), os.environ.get("JOURNAL_WEBHOOK", "")
    h, l = float(df['high'].iloc[-1]), float(df['low'].iloc[-1])
    msg_closed = ""
    
    # 1. EVALUATE EXISTING TRADE
    if led["open_trade"]:
        ot = led["open_trade"]
        sl_hit = (ot["dir"]=="LONG" and l<=ot["sl"]) or (ot["dir"]=="SHORT" and h>=ot["sl"])
        tp_hit = (ot["dir"]=="LONG" and h>=ot["tp"]) or (ot["dir"]=="SHORT" and l<=ot["tp"])
        
        if sl_hit or tp_hit or action=="FLAT" or (action!=ot["dir"] and action!="FLAT"):
            if tp_hit: pnl = (ot["tp"]-ot["entry"] if ot["dir"]=="LONG" else ot["entry"]-ot["tp"]) * ot["qty"] * cfg.point_value
            elif sl_hit: pnl = (ot["sl"]-ot["entry"] if ot["dir"]=="LONG" else ot["entry"]-ot["sl"]) * ot["qty"] * cfg.point_value
            else: pnl = (price-ot["entry"] if ot["dir"]=="LONG" else ot["entry"]-price) * ot["qty"] * cfg.point_value
            
            pnl -= (ot["qty"] * cfg.commission_rt); led["equity"] += pnl
            if led["equity"] > led["peak_equity"]: led["peak_equity"] = led["equity"]
            
            out = "WIN ✅" if pnl > 0 else "LOSS ❌"
            msg_closed = f"\n**Trade Closed:** {out} (${pnl:,.2f})"
            if j_url:
                requests.post(j_url, data={"content": f"📓 **JOURNAL**\n**Closed:** {ot['dir']}\n**Result:** {out} (${pnl:,.2f})\n**Balance:** ${led['equity']:,.2f}"})
            led["open_trade"] = None

    # 2. OPEN NEW TRADE (CRITICAL FIX: Only if not already holding)
    if not led["open_trade"] and action != "FLAT" and contracts > 0:
        sl = price - sl_pts if action == "LONG" else price + sl_pts
        tp = price + tp_pts if action == "LONG" else price - tp_pts
        led["open_trade"] = {"dir": action, "entry": price, "sl": sl, "tp": tp, "qty": contracts}

    json.dump(led, open(L_FILE, 'w'), indent=4)

    # 3. CHARTING
    cdf = df.tail(40).copy(); cdf.columns = [c.capitalize() for c in cdf.columns]
    s = mpf.make_mpf_style(marketcolors=mpf.make_marketcolors(up='#00ffaa', down='#ff0055', edge='inherit', wick='inherit', volume='in', ohlc='i'), gridstyle=':', base_mpf_style='nightclouds')
    c_path = "live_chart.png"
    
    if led["open_trade"]:
        hl = dict(hlines=[led['open_trade']['entry'], led['open_trade']['tp'], led['open_trade']['sl']], colors=['white', '#00ff00', '#ff0000'], linestyle='--', linewidths=1.5)
        mpf.plot(cdf, type='candle', style=s, hlines=hl, savefig=c_path, title=f"GHOST | HOLDING {led['open_trade']['dir']}")
    elif action != "FLAT":
        hl = dict(hlines=[price, price+tp_pts if action=="LONG" else price-tp_pts, price-sl_pts if action=="LONG" else price+sl_pts], colors=['white', '#00ff00', '#ff0000'], linestyle='--', linewidths=1.5)
        mpf.plot(cdf, type='candle', style=s, hlines=hl, savefig=c_path, title=f"GHOST | {action} (NEW)")
    else:
        mpf.plot(cdf, type='candle', style=s, savefig=c_path, title="GHOST | FLAT")

    # 4. DISCORD
    if d_url:
        rz = "GREEN" if get_risk(led['equity'], led['peak_equity'], cfg.risk_per_trade_pct, cfg.combine_max_dd_pct) >= cfg.risk_per_trade_pct * 0.99 else "AMBER/RED"
        st = f"HOLDING {led['open_trade']['dir']}" if led["open_trade"] else action
        msg = f"🚨 **GHOST SENTINEL** 🚨\n**Action:** `{st}` {cfg.name}\n**Equity:** `${led['equity']:,.2f}`\n**Zone:** `{rz}`{msg_closed}\n_Conf: {conf:.5f} | Thr: {thr:.5f}_"
        try: requests.post(d_url, data={"content": msg}, files={"file": open(c_path, 'rb')})
        except: pass

def run_full_pipeline(df: pd.DataFrame, instrument="MNQ"):
    cfg = MNQ_CONFIG if instrument == "MNQ" else ES_CONFIG
    f_df, f_cols = build_ml_features(df)
    
    # Check for brain
    b_file = "ai_brain.json"
    brain = json.load(open(b_file)) if os.path.exists(b_file) else {"epsilon": 1.2, "confidence_multiplier": 0.5} # Aggressive default
    
    conf, thr = train_and_predict(f_df, f_cols, brain["epsilon"], brain["confidence_multiplier"])
    act = "LONG" if conf > thr else "SHORT" if conf < -thr else "FLAT"
    
    prc = float(df['close'].iloc[-1])
    atr = float(f_df['atr'].iloc[-1]) * prc
    sl_pts, tp_pts = round(cfg.atr_stop_mult * atr, 4), round(cfg.atr_tp_mult * atr, 4)
    
    eq = json.load(open("ledger.json"))["equity"] if os.path.exists("ledger.json") else cfg.account_size
    qty = int(np.floor((eq * cfg.risk_per_trade_pct) / (sl_pts * cfg.point_value))) if act != "FLAT" else 0
    
    process_trade(df, act, qty, prc, sl_pts, tp_pts, abs(conf), thr, cfg)
