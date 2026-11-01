fn main() {
    match plum_next::org::list_repos("Rime-Next") {
        Ok(repos) => println!("列表拉取成功: {} 个仓库", repos.len()),
        Err((msg, network)) => println!("失败({network}): {msg}"),
    }
}
