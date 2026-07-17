# bao — unified library entry

Single public Cargo package for embedding Bao.

**Always on (no product feature splits):** SpiderMonkey engine · servo browser · Node/Bun API · CDP · Stealth.

```toml
[dependencies]
bao = { path = "path/to/bao/src/bao" }
```

```rust
use bao::{BaoConfig, BaoRuntime, PageConfig, ScreenshotFormat, StealthProfile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = BaoRuntime::new(BaoConfig::default())?;
    let _ = StealthProfile::firefox_default();
    let _ = ScreenshotFormat::Png;
    let _ = PageConfig::default();
    Ok(())
}
```

Internal crates (`bao_browser`, `bao_engine`, …) are monorepo implementation details — depend on `bao` only.
