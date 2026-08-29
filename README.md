# render_sbgn_rs

> [!IMPORTANT]
> This repository is archived. Active development has moved to
> [cannin/render_sbgn](https://github.com/cannin/render_sbgn). The Rust
> implementation is now maintained in the monorepo's
> [rust directory](https://github.com/cannin/render_sbgn/tree/main/rust).

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
