# /// script
# requires-python = ">=3.12"
# dependencies = ["ta-lib==0.8.3", "numpy==2.3.5"]
# ///
"""Writes talib.json: TA-Lib's indicators over generated prices, which the engine's indicator tests compare with.

Run from this directory with `uv run generate_talib.py`. Python is used only to produce these fixtures: TA-Lib is the reference
the design names, and its Python wheel bundles the C library.
"""

import json
import pathlib

import numpy as np
import talib
from talib import abstract

ROWS = 300


def bars(seed: int, start: float, volatility: float) -> dict[str, list[float]]:
    """Daily bars with fat-tailed returns, every high above and every low below the open and close."""
    rng = np.random.default_rng(seed)
    returns = rng.standard_t(3, ROWS) * volatility
    close = start * np.exp(np.cumsum(returns))
    open_ = np.concatenate([[start], close[:-1]]) * np.exp(rng.normal(0, volatility / 4, ROWS))
    high = np.maximum(open_, close) * np.exp(np.abs(rng.normal(0, volatility / 2, ROWS)) + 1e-6)
    low = np.minimum(open_, close) * np.exp(-np.abs(rng.normal(0, volatility / 2, ROWS)) - 1e-6)
    volume = rng.integers(1_000, 5_000_000, ROWS).astype(float)
    return {"open": open_.tolist(), "high": high.tolist(), "low": low.tolist(), "close": close.tolist(), "volume": volume.tolist()}


def plain(values: np.ndarray) -> list[float | None]:
    return [None if np.isnan(v) else float(v) for v in values]


def flat(b: dict[str, list[float]], start: int, rows: int) -> dict[str, list[float]]:
    """The same bars with a stretch where the price does not move, as for a halted stock."""
    out = {k: list(v) for k, v in b.items()}
    level = out["close"][max(start - 1, 0)]
    for k in ("open", "high", "low", "close"):
        out[k][start : start + rows] = [level] * rows
    return out


def outputs(b: dict[str, list[float]], skip: frozenset[str] = frozenset()) -> list[dict[str, object]]:
    h, l, c, v = (np.array(b[k]) for k in ("high", "low", "close", "volume"))
    out: list[dict[str, object]] = []

    def add(function: str, params: dict[str, float], results: dict[str, np.ndarray]) -> None:
        if function in skip:
            return
        out.append({"function": function, "params": params, "outputs": {k: plain(r) for k, r in results.items()}})

    for period in (2, 5, 20, 200):
        add("sma", {"period": period}, {"real": talib.SMA(c, period)})
        add("ema", {"period": period}, {"real": talib.EMA(c, period)})
    for period in (2, 9, 50):
        add("wma", {"period": period}, {"real": talib.WMA(c, period)})
    for period, deviations in ((20, 2.0), (5, 1.5)):
        upper, middle, lower = talib.BBANDS(c, period, deviations, deviations, 0)
        add("bollinger", {"period": period, "deviations": deviations}, {"upper": upper, "middle": middle, "lower": lower})
    for period in (2, 14):
        add("rsi", {"period": period}, {"real": talib.RSI(c, period)})
    for fast, slow, signal in ((12, 26, 9), (3, 10, 16)):
        macd, sig, hist = talib.MACD(c, fast, slow, signal)
        add("macd", {"fast": fast, "slow": slow, "signal": signal}, {"macd": macd, "signal": sig, "histogram": hist})
    for period in (1, 5, 14):
        add("atr", {"period": period}, {"real": talib.ATR(h, l, c, period)})
    for k, k_smooth, d in ((14, 3, 3), (5, 1, 1)):
        slow_k, slow_d = talib.STOCH(h, l, c, k, k_smooth, 0, d, 0)
        add("stochastic", {"k": k, "k_smooth": k_smooth, "d": d}, {"k": slow_k, "d": slow_d})
    # VWAP over every row, then restarting every 50 rows, as for sessions of intraday bars.
    add("vwap", {"session": 0}, {"real": abstract.VWAP({"high": h, "low": l, "close": c, "volume": v})})
    sessions = [abstract.VWAP({"high": h[i : i + 50], "low": l[i : i + 50], "close": c[i : i + 50], "volume": v[i : i + 50]}) for i in range(0, ROWS, 50)]
    add("vwap", {"session": 50}, {"real": np.concatenate(sessions)})
    return out


cases = []
for name, seed, start, volatility in (("hundred", 1, 100.0, 0.02), ("ten-thousand", 2, 12_000.0, 0.015), ("sub-penny", 3, 0.000_5, 0.05)):
    b = bars(seed, start, volatility)
    cases.append({"name": name, **b, "results": outputs(b)})
# Bars that start without movement, where an RSI has neither gains nor losses
# to average, and forty more without movement later, where Wilder's smoothing
# still carries the earlier ones. A stochastic over a flat window has no value
# in parallax and reads 0 in TA-Lib, which the engine's own tests cover.
b = flat(flat(bars(4, 50.0, 0.02), 0, 30), 100, 40)
cases.append({"name": "flat-stretch", **b, "results": outputs(b, frozenset({"stochastic"}))})

document = {"talib": talib.__version__, "cases": cases}
path = pathlib.Path(__file__).with_name("talib.json")
path.write_text(json.dumps(document, separators=(",", ":")) + "\n")
print(f"wrote {path.name}: {len(cases)} cases, TA-Lib {talib.__version__}")
