# render_sbgn_rs

Rust CLI for rendering SBGNML diagrams to PNG and SVG.

## Compile

```bash
cargo build --release
```

The binary will be at:

```
target/release/render_sbgn_rs
```

## Run

```bash
./target/release/render_sbgn_rs draw_sbgnml \
  --input examples/sbgn/foo.sbgn \
  --output out.png \
  --padding 10
```

`--input` is required. PNG and SVG outputs are written by default using the `--output` path to derive the SVG filename.
