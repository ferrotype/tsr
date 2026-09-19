# Supplemental producer provenance

The nine `.py` files here are byte-for-byte copies of the historical drivers
retained in the named local captures. They were originally temporary scripts,
not committed producers. Their absolute checkout/output paths are preserved so
the recorded `producer_sha256` values remain verifiable. Do not edit or run them
as the supported interface. `manifest.json` binds each driver, selection and
historical report; `replay-audit.json` records exact replay of all nine reports
through the current repository comparator, with no Rust or Go execution.

Use `scripts/s08_p5_recheck.py` for new captures and replay. It authenticates the
requests, native observations, driver, executable, source snapshot and raw case
results. Resume keeps the captured executable and records whether the working
sources still match. The committed selections preserve control order. Expected
observations are comparison inputs only; Rust receives the original request.

For example, replay the historical 637/95/5 figures or run the same selection on
current sources:

```sh
python3 scripts/s08_p5_recheck.py --output target/s08/p5-corpus-display-recheck-01 --replay
python3 scripts/s08_p5_recheck.py --output target/s08/p5-corpus-return-recheck-01 --replay
python3 scripts/s08_p5_recheck.py --output target/s08/p5-corpus-display-recheck-04 --replay
python3 scripts/s08_p5_recheck.py \
  --control target/s08/p5-corpus-display-recheck-01 \
  --selection tools/s08/results/p5-corpus/producers/p5-corpus-return-recheck-01.selection.json \
  --output target/s08/p5-return-new
```

Replay needs the named raw capture (retained locally, not copied into Git).
Fresh corpus requests and observations can be regenerated with the commands in
`docs/S08-P5.md`; the committed selections also work against that full control.
Reproducing an old result requires its recorded source snapshot and build
configuration. Running current code establishes a new result, not reproduction
of the old binary. Capture counts must never be added into a full-corpus total.
