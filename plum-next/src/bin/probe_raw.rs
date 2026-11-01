//! 原始错误链:看 reqwest 失败的真实原因(connect/DNS/TLS/代理)。
fn main() {
    let client = reqwest::blocking::Client::builder()
        .user_agent("weasel-marketplace")
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap();
    match client
        .get("https://api.github.com/orgs/Rime-Next/repos?per_page=1")
        .header("Accept", "application/vnd.github+json")
        .send()
    {
        Ok(resp) => println!("OK {} ({} bytes)", resp.status(), resp.content_length().unwrap_or(0)),
        Err(e) => {
            println!("失败: {e}");
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                println!("  ↳ {s}");
                src = s.source();
            }
        }
    }
}
