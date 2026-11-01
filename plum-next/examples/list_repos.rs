// 探针：验证「设置 → 下载」页背后的列表拉取是否真的能拿到仓库。
// 跑法：cargo run --release -p plum-next --example list_repos
fn main() {
    let started = std::time::Instant::now();
    match plum_next::org::list_repos(plum_next::org::ORG) {
        Ok(repos) => {
            println!(
                "列表拉取成功: {} 个仓库，耗时 {} ms，组织 {}",
                repos.len(),
                started.elapsed().as_millis(),
                plum_next::org::ORG
            );
            for r in repos.iter().take(8) {
                println!(
                    "  - {}{}",
                    r.name,
                    r.description
                        .as_deref()
                        .map(|d| format!("：{d}"))
                        .unwrap_or_default()
                );
            }
        }
        Err((msg, network)) => println!("失败(network={network}): {msg}"),
    }
}
