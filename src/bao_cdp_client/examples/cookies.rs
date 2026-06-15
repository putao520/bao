//! # 示例:Cookie 管理 — 添加 / 查询 / 删除
//!
//! 演示 [`Cookie`] 类型的构造与序列化:
//! - 通过 builder API 构造(`Cookie::new(...).with_domain(...).with_secure(...)`)
//! - JSON 序列化(可持久化或通过 CDP `Network.setCookie` 发送)
//! - 反序列化恢复
//!
//! ## 与 CDP 的对应关系
//!
//! [`Cookie`] 字段映射 CDP `Network.Cookie` / `Network.setCookie` 参数:
//!
//! | Cookie 字段     | CDP 字段          |
//! |----------------|-------------------|
//! | `name`         | `name`            |
//! | `value`        | `value`           |
//! | `url`          | `url`             |
//! | `domain`       | `domain`          |
//! | `path`         | `path`            |
//! | `expires`      | `expires`         |
//! | `http_only`    | `httpOnly`        |
//! | `secure`       | `secure`          |
//! | `same_site`    | `sameSite`        |
//!
//! ## 完整代码(伪)
//!
//! ```no_run
//! use bao_cdp_client::types::Cookie;
//! use bao_cdp_client::Browser;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let browser = Browser::connect("memory://bao")?;
//!     let mut transport = browser.build_in_memory_transport(my_bridge)?;
//!
//!     // 添加 Cookie
//!     let cookie = Cookie::new("session", "abc123")
//!         .with_domain("example.com")
//!         .with_path("/")
//!         .with_secure(true)
//!         .with_http_only(true)
//!         .with_same_site("Lax");
//!
//!     transport.send_command(
//!         "Network.setCookie",
//!         serde_json::to_value(&cookie)?,
//!         None,
//!     )?;
//!
//!     // 查询所有 Cookie
//!     let resp = transport.send_command(
//!         "Network.getCookies",
//!         serde_json::json!({ "urls": ["https://example.com"] }),
//!         None,
//!     )?;
//!     let cookies: Vec<Cookie> = serde_json::from_value(resp["cookies"].clone())?;
//!     assert!(cookies.iter().any(|c| c.name == "session"));
//!
//!     Ok(())
//! }
//! ```
//!
//! ## 运行
//!
//! ```sh
//! cargo run --example cookies -p bao_cdp_client
//! ```
//!
//! @trace REQ-BAO-API-008 [level:library]

use bao_cdp_client::types::Cookie;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 用 builder API 构造一个完整 Cookie
    let cookie = Cookie::new("session", "abc123")
        .with_domain("example.com")
        .with_path("/")
        .with_secure(true)
        .with_http_only(true)
        .with_same_site("Lax");

    println!("Cookie: {:#?}", cookie);

    // 序列化为 JSON(对应 CDP Network.setCookie 参数)
    let json = serde_json::to_string_pretty(&cookie)?;
    println!("\nSerialized JSON:\n{}", json);

    // 反序列化恢复
    let back: Cookie = serde_json::from_str(&json)?;
    assert_eq!(cookie, back);
    println!("\nRound-trip OK: name={}, value={}", back.name, back.value);

    // 只含必填字段的 Cookie(name + value)
    let minimal = Cookie::new("flag", "1");
    let min_json = serde_json::to_string(&minimal)?;
    println!("\nMinimal cookie JSON: {}", min_json);
    // 可选字段为 None 时被 skip_serializing_if 跳过

    println!("\nExample completed successfully.");
    Ok(())
}
