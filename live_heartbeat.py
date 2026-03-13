import os, sys, logging, pandas as pd, yfinance as yf
from datetime import datetime, timezone
try: from ghost_sentinel_v5_ml import run_full_pipeline
except ImportError: sys.exit(1)

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")

def main():
    logging.info("Booting Engine...")
    b = pd.read_csv("NSXUSD_1H.csv", index_col=0, parse_dates=True)
    b.index = b.index.tz_localize("UTC") if b.index.tzinfo is None else b.index.tz_convert("UTC")
    b.columns = [c.lower().strip() for c in b.columns]
    
    r = yf.download("NQ=F", period="1mo", interval="1h", progress=False, auto_adjust=True)
    r.columns = [c[0].lower().strip() if isinstance(r.columns, pd.MultiIndex) else c.lower().strip() for c in r.columns]
    r.index = pd.to_datetime(r.index, utc=True)
    
    m = pd.concat([b, r.iloc[:-1]]); m = m[~m.index.duplicated(keep='last')].sort_index()
    run_full_pipeline(m, "MNQ")

if __name__ == "__main__": main()
