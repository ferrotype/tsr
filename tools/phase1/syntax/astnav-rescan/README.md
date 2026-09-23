This access-only Go overlay calls the pinned private navigation operations. To
reproduce the five observations without modifying the frozen fixture:

```sh
PATH="/Users/cristian/.local/share/mise/installs/go/1.27.1/bin:$PATH" \
  python3 scripts/phase1_navigation_rescan.py --output target/navigation-rescan-review
```

The output directory must be new. `--write` explicitly refreshes both native
observations/provenance and the derived Rust TSV after review. Without it, the
command compares measured operation results against the retained observations
and leaves committed files unchanged. Go host metadata may differ across hosts;
request, pinned source and operation results must agree.
