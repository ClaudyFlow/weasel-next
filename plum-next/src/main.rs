//! PlumNext 命令行入口。
//!
//! 用法（与原版 `rime-install` 对齐）：
//!
//! ```text
//! plum-next                        # 等价于 :preset
//! plum-next :preset                # 预设配方清单
//! plum-next luna-pinyin emoji      # 指定包
//! plum-next lotem/rime-zhung@master:my-recipe:a=1
//! plum-next --list-mirrors         # 列出内置 + 自定义镜像
//! plum-next --sniff                # 只做镜像嗅探并打印排序
//! plum-next --add-mirror my <git前缀> [raw前缀]
//!                                     # 添加/覆盖自定义镜像并立即探测
//! plum-next --rime-dir <dir>       # 指定 Rime 用户目录
//! plum-next --mirror ghproxy       # 强制指定镜像（不做嗅探）
//! ```

use std::path::PathBuf;

use plum_next::install;
use plum_next::mirror::{self, Mirror, Picker};
use plum_next::progress;
use plum_next::recipe;
use plum_next::target::{self, Target};

const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

struct Args {
    rime_dir: PathBuf,
    plum_dir: PathBuf,
    force_mirror: Option<String>,
    list_mirrors: bool,
    sniff_only: bool,
    add_mirror: Option<AddMirror>,
    progress_file: Option<PathBuf>,
    no_update: bool,
    select: bool,
    no_deploy: bool,
    targets: Vec<String>,
    // 「拉取 + 搬运」子命令
    relocate: bool,
    relocate_url: Option<String>,
    relocate_root: Option<String>,
    relocate_branch: Option<String>,
    relocate_parts: Option<String>,
    relocate_force: bool,
    proxy: Option<String>,
}

/// `--add-mirror <id> <git前缀> [raw前缀]` 的解析结果。
struct AddMirror {
    id: String,
    git: String,
    raw: Option<String>,
}

fn parse_args() -> Args {
    let mut rime_dir = None;
    let mut plum_dir = None;
    let mut force_mirror = None;
    let mut list_mirrors = false;
    let mut sniff_only = false;
    let mut add_mirror = None;
    let mut progress_file = None;
    let mut no_update = false;
    let mut select = false;
    let mut no_deploy = false;
    let mut targets = Vec::new();
    let mut relocate = false;
    let mut relocate_url = None;
    let mut relocate_root = None;
    let mut relocate_branch = None;
    let mut relocate_parts = None;
    let mut relocate_force = false;
    let mut proxy = None;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--list-mirrors" => list_mirrors = true,
            "--sniff" => sniff_only = true,
            "--no-update" => no_update = true,
            "--select" => select = true,
            "--no-deploy" => no_deploy = true,
            "--progress-file" => progress_file = it.next().map(PathBuf::from),
            "--rime-dir" => rime_dir = it.next(),
            "--plum-dir" => plum_dir = it.next(),
            "--mirror" => force_mirror = it.next(),
            "--add-mirror" => {
                let Some(id) = it.next() else {
                    eprintln!("--add-mirror 需要一个 id 参数");
                    std::process::exit(2);
                };
                let Some(git) = it.next() else {
                    eprintln!("--add-mirror 需要一个 git 前缀参数");
                    std::process::exit(2);
                };
                let raw = it.next();
                add_mirror = Some(AddMirror {
                    id,
                    git,
                    raw,
                });
            }
            "relocate" => relocate = true,
            "--url" => relocate_url = it.next(),
            "--root" => relocate_root = it.next(),
            "--branch" => relocate_branch = it.next(),
            "--parts" => relocate_parts = it.next(),
            "--force" => relocate_force = true,
            "--proxy" => proxy = it.next(),
            other if other.starts_with("--rime-dir=") => {
                rime_dir = Some(other["--rime-dir=".len()..].to_string())
            }
            other if other.starts_with("--plum-dir=") => {
                plum_dir = Some(other["--plum-dir=".len()..].to_string())
            }
            other if other.starts_with("--mirror=") => {
                force_mirror = Some(other["--mirror=".len()..].to_string())
            }
            other if other.starts_with("--progress-file=") => {
                progress_file = Some(PathBuf::from(&other["--progress-file=".len()..]))
            }
            other => targets.push(other.to_string()),
        }
    }
    if targets.is_empty() && !list_mirrors && !sniff_only && add_mirror.is_none() {
        targets.push(":preset".to_string());
    }

    let rime_dir = rime_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_rime_dir);
    let plum_dir = plum_dir.map(PathBuf::from).unwrap_or_else(default_plum_dir);
    Args {
        rime_dir,
        plum_dir,
        force_mirror,
        list_mirrors,
        sniff_only,
        add_mirror,
        progress_file,
        no_update,
        select,
        no_deploy,
        targets,
        relocate,
        relocate_url,
        relocate_root,
        relocate_branch,
        relocate_parts,
        relocate_force,
        proxy,
    }
}

fn print_help() {
    println!(
        "PlumNext —— 東風破 /plum/ 的 Rust 重写\n\
         \n\
         用法:\n\
         \x20 plum-next [选项] <目标>...\n\
         \n\
         目标:\n\
         \x20 :preset / :extra / :all   预设清单\n\
         \x20 <包名>                    如 luna-pinyin、emoji\n\
         \x20 <用户>/<仓库>[@分支][:配方[:选项]]\n\
         \x20 <用户>/<仓库>/xxx-packages.conf\n\
         \n\
         选项:\n\
         \x20 --rime-dir <目录>    Rime 用户目录（默认 %APPDATA%\\Rime-Next）\n\
         \x20 --plum-dir <目录>    配方包缓存目录（mirrors.txt 也在这里）\n\
         \x20 --select              交互式挑选要安装的配方包\n\
         \x20 --mirror <id>         强制使用某个镜像，不做嗅探\n\
         \x20 --list-mirrors       列出内置 + 自定义镜像\n\
         \x20 --sniff              只嗅探镜像（含自定义）并打印排序\n\
         \x20 --add-mirror <id> <git前缀> [raw前缀]\n\
         \x20                        添加/覆盖自定义镜像，写 mirrors.txt 并立即探测\n\
         \x20 --no-update          已存在的包不拉取更新
         \x20 --no-deploy          装完不自动触发重新部署
         \n\
         \x20 relocate 子命令（整仓搬运）：
         \x20   plum-next relocate --url <GitHub仓库URL> --root <用户根目录>
         \x20     [--branch <分支>] [--parts part1,part2] [--force] [--proxy <代理>]
         \x20     把仓库根层的 part1/part2 整体搬运到用户根目录（删临时仓库目录）"
    );
}

/// Weasel-Next 的用户目录：与安装器/其他组件同一套解析
/// （`WEASEL_NEXT_USER_DIR` > `<程序目录>\userdir.txt` > `%APPDATA%\Rime-Next`）。
///
/// 以前这里自己拼 `%APPDATA%`：用户装了自选的用户文件夹后，plum 会把方案装到旧位置，
/// 设置界面看不到新方案（2026-10-29 修）。
fn default_rime_dir() -> PathBuf {
    weasel_common::process::user_data_dir().unwrap_or_else(|| PathBuf::from("rime"))
}

/// 配方包缓存：默认放在本地应用数据下，不污染用户目录。
fn default_plum_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Rime-Next").join("plum")
}

fn main() {
    let args = parse_args();

    // 进度文件：给了就先删掉旧文件（读方按"取最后一行"消费，重跑时留旧行会串味）
    if let Some(pf) = &args.progress_file {
        let _ = std::fs::remove_file(pf);
        progress::set_sink(Some(pf));
    }

    // 「拉取 + 搬运」子命令走独立流程，不进下面的逐包挑选。
    if args.relocate {
        run_relocate(&args);
        return;
    }

    if args.list_mirrors {
        for m in mirror::builtin() {
            println!("{:<16} {}", m.id, m.label);
        }
        return;
    }

    if args.sniff_only {
        progress::emit("sniff", 0.1, "开始嗅探");
        println!("嗅探中（超时 {} ms）…", PROBE_TIMEOUT.as_millis());
        for probe in mirror::sniff(&mirror::builtin(), PROBE_TIMEOUT) {
            match probe.latency {
                Some(d) => println!("  {:<16} {:>6} ms  {}", probe.mirror.id, d.as_millis(), probe.mirror.label),
                None => println!("  {:<16} {:>6}     {}", probe.mirror.id, "不可达", probe.mirror.label),
            }
        }
        progress::emit("sniff", 1.0, "嗅探完成");
        return;
    }

    let mirrors: Vec<Mirror> = match &args.force_mirror {
        Some(id) => match mirror::by_id(id) {
            Some(m) => vec![m],
            None => {
                eprintln!("未知镜像: {id}（用 --list-mirrors 查看）");
                std::process::exit(2);
            }
        },
        None if mirror::load_state(&args.plum_dir).is_some() => {
            // 快路径：上次成功过的镜像还在，就不用每次都嗅探（省掉一整轮建连探测）。
            // 它真挂了的话 Picker 会自动往下试，试完再兜底嗅探（见下面的分支顺序）。
            let id = mirror::load_state(&args.plum_dir).unwrap();
            println!("沿用上次成功的镜像: {id}（失败会自动切换）");
            mirror::builtin()
        }
        None => {
            println!("嗅探镜像…");
            let probes = mirror::sniff(&mirror::builtin(), PROBE_TIMEOUT);
            for p in &probes {
                match p.latency {
                    Some(d) => println!("  {:<16} {:>6} ms", p.mirror.id, d.as_millis()),
                    None => println!("  {:<16} 不可达", p.mirror.id),
                }
            }
            let reachable: Vec<Mirror> = probes
                .into_iter()
                .filter(|p| p.latency.is_some())
                .map(|p| p.mirror)
                .collect();
            if reachable.is_empty() {
                println!("所有镜像都不可达，仍按原顺序尝试（可能失败）");
                mirror::builtin()
            } else {
                progress::emit("sniff", 1.0, "嗅探完成");
                reachable
            }
        }
    };

    let preferred = args
        .force_mirror
        .clone()
        .or_else(|| mirror::load_state(&args.plum_dir));

    let mut files_updated = 0usize;
    let mut packages_done = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut last_mirror: Option<String> = None;

    // 先把所有目标展开成包列表（`--select` 时才能一次性列出来让用户挑）
    let mut all_packages: Vec<String> = Vec::new();
    for raw in &args.targets {
        for p in resolve_targets(raw, &args, &mirrors, &preferred) {
            if !all_packages.contains(&p) {
                all_packages.push(p);
            }
        }
    }
    if args.select {
        all_packages = select_packages(&all_packages);
    }
    let total = all_packages.len();
    for (idx, pkg) in all_packages.iter().enumerate() {
        match install_package(pkg, &args, &mirrors, &preferred, &mut last_mirror, idx, total) {
            Ok(n) => {
                files_updated += n;
                packages_done += 1;
            }
            Err(e) => failures.push(format!("{pkg}: {e}")),
        }
    }

    if let Some(id) = &last_mirror {
        let _ = mirror::save_state(&args.plum_dir, id);
    }

    println!();
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("失败 {}", f);
        }
    }
    if files_updated == 0 {
        println!("没有文件更新。");
    } else {
        println!(
            "更新 {} 个文件（来自 {} 个配方包）→ {}",
            files_updated,
            packages_done,
            args.rime_dir.display()
        );
    }
    if !failures.is_empty() {
        progress::emit("done", 0.0, &format!("失败 {} 个包", failures.len()));
        std::process::exit(1);
    }
    progress::emit("done", 1.0, &format!("更新 {} 个文件", files_updated));
    maybe_request_deploy(&args, files_updated);
}

/// 装完顺手触发一次重新部署：调同目录的 `Weasel-Next-Setting.exe /deploy`。
///
/// 部署必须由掌握引擎生死的 broker 做（设置程序自己拉起第二个引擎只会 AlreadyRunning），
/// 所以这里借道设置程序的 `/deploy` 命令行，而不是自己去找 broker 窗口 —— 省得在
/// PlumNext 里再拖一套 Win32 依赖。
fn maybe_request_deploy(args: &Args, files_updated: usize) {
    if files_updated == 0 || args.no_deploy {
        return;
    }
    let setting = std::env::current_exe()
        .ok()
        .as_ref()
        .and_then(|exe| exe.parent())
        .map(|dir| dir.join("Weasel-Next-Setting.exe"))
        .filter(|p| p.exists());
    let Some(setting) = setting else {
        println!("（同目录没有 Weasel-Next-Setting.exe，跳过自动部署；请手动重新部署)");
        return;
    };
    match std::process::Command::new(setting).arg("/deploy").spawn() {
        Ok(_) => println!("已请求重新部署…"),
        Err(e) => eprintln!("请求重新部署失败: {e}"),
    }
}

/// 「拉取 + 搬运」子命令：clone 一个仓库，把根层的 part1/part2 整体搬到用户根目录。
///
/// 用法：`plum-next relocate --url <GitHub 仓库 URL> --root <用户根目录>
///        [--branch <分支>] [--parts part1,part2] [--force] [--proxy <代理>]`
fn run_relocate(args: &Args) {
    let Some(url) = &args.relocate_url else {
        eprintln!("relocate 需要 --url <GitHub 仓库 URL>");
        std::process::exit(2);
    };
    let Some(root) = &args.relocate_root else {
        eprintln!("relocate 需要 --root <用户根目录>");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);
    let parts: Vec<String> = args
        .relocate_parts
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_else(|| vec!["part1".to_string(), "part2".to_string()]);
    let opts = plum_next::fetch::FetchOpts {
        proxy: args.proxy.clone(),
    };
    let emit = |phase: &str, pct: f64, detail: &str| progress::emit(phase, pct, detail);
    match plum_next::relocate::pull_and_relocate(
        &root,
        url,
        args.relocate_branch.as_deref(),
        &parts,
        args.relocate_force,
        &opts,
        emit,
    ) {
        Ok(r) => {
            println!(
                "完成：仓库 {} 的 {:?} 已搬运到 {}",
                r.repo_name,
                r.parts,
                root.display()
            );
        }
        Err(e) => {
            eprintln!("失败：{e}");
            progress::emit("done", 0.0, &e);
            std::process::exit(1);
        }
    }
}

/// 把一个目标展开成"包订单"列表（清单类目标会展开成多个包）。
fn resolve_targets(
    raw: &str,
    args: &Args,
    mirrors: &[Mirror],
    preferred: &Option<String>,
) -> Vec<String> {
    if !target::is_configuration_target(raw) {
        return vec![raw.to_string()];
    }

    // 远端清单：完整 URL 或 `<user>/<repo>[@branch]/xxx-packages.conf`
    let remote_url = if raw.starts_with("https://") {
        Some(raw.to_string())
    } else if raw.ends_with(".conf") && raw.contains('/') {
        target::expand_configuration_url(raw)
    } else {
        None
    };
    if let Some(url) = remote_url {
        match http_get(&url) {
            Ok(text) => return target::parse_package_conf(&text),
            Err(e) => eprintln!("取不到远端清单: {url}（{e}）"),
        }
        eprintln!("取不到远端清单: {url}");
        return Vec::new();
    }

    // `:preset` 这类：先找本地，再走镜像下载，最后用内置兜底清单
    if let Some(path) = target::local_configuration_path(&args.plum_dir, raw) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            return target::parse_package_conf(&text);
        }
    }
    if let Some(name) = target::configuration_file_name(raw) {
        let repo_path = format!("rime/plum/master/{name}");
        let mut picker = Picker::new(mirrors.to_vec(), preferred.as_deref());
        if let Ok((text, used)) = picker.attempt(|m| http_get(&mirror::raw_url(m, &repo_path))) {
            println!("清单来源: {}", used.label);
            let _ = std::fs::create_dir_all(&args.plum_dir);
            let _ = std::fs::write(args.plum_dir.join(&name), &text);
            return target::parse_package_conf(&text);
        }
    }
    println!("联网取不到 {raw}，改用内置兜底清单");
    target::FALLBACK_PRESET.iter().map(|s| s.to_string()).collect()
}

/// 取一段文本（`*-packages.conf`）。
fn http_get(url: &str) -> Result<String, String> {
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}

/// 交互式选包（原版 `--select`）：把展开出来的包列出来让用户挑。
fn select_packages(packages: &[String]) -> Vec<String> {
    if packages.is_empty() {
        return Vec::new();
    }
    println!("\n可安装的配方包：");
    for (i, p) in packages.iter().enumerate() {
        println!("  {:>2}. {}", i + 1, p);
    }
    println!("\n输入序号挑选（空格分隔，如 `1 3`；`a` 全选；回车取消）：");
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return Vec::new();
    }
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    let picked = parse_selection(line, packages);
    picked
}

/// 解析用户输入的序号串：空格分隔的序号 / `a`(全选) / 空(取消)。
/// 越界与重复序号会被忽略（重复只保留一次）。
fn parse_selection(input: &str, packages: &[String]) -> Vec<String> {
    let line = input.trim();
    if line.is_empty() {
        return Vec::new();
    }
    if line == "a" || line == "all" {
        return packages.to_vec();
    }
    let mut picked = Vec::new();
    for tok in line.split_whitespace() {
        if let Ok(n) = tok.parse::<usize>() {
            // 序号从 1 开始；`0` 会下溢，必须挡掉
            let Some(index) = n.checked_sub(1) else {
                continue;
            };
            if let Some(p) = packages.get(index) {
                if !picked.contains(p) {
                    picked.push(p.clone());
                }
            }
        }
    }
    picked
}

/// 拉包 + 按配方安装，返回更新的文件数。
///
/// `pkg_index`/`pkg_total` 用于计算整体进度（第几个包 / 共几个包），
/// 映射到 [0,1] 区间，再按包内阶段细分。
fn install_package(
    pkg: &str,
    args: &Args,
    mirrors: &[Mirror],
    preferred: &Option<String>,
    last_mirror: &mut Option<String>,
    pkg_index: usize,
    pkg_total: usize,
) -> Result<usize, String> {
    let t: Target = target::parse_target(pkg).ok_or_else(|| "无法解析目标".to_string())?;
    let package_dir = args
        .plum_dir
        .join("package")
        .join(&t.user)
        .join(&t.package_name);

    // 包内阶段权重：clone/fetch 最重，安装次之，补丁最后
    let stage_weight = |s: &str| -> f64 {
        match s {
            "clone" | "fetch" => 0.5,
            "download" => 0.7,
            "install" => 0.9,
            "patch" => 1.0,
            _ => 0.5,
        }
    };
    let base = if pkg_total > 0 {
        pkg_index as f64 / pkg_total as f64
    } else {
        0.0
    };

    // ── 拉取：按镜像顺序尝试，失败自动切换 ──
    let mut picker = Picker::new(mirrors.to_vec(), preferred.as_deref());
    let mut clone_done = false;
    let mut clone_used: Option<Mirror> = None;
    let (action, used) = picker.attempt(|m| {
        if package_dir.exists() && args.no_update {
            clone_done = true;
            return Ok("已存在".to_string());
        }
        let url = mirror::git_url(m, &t.package);
        progress::emit(
            "clone",
            base + stage_weight("clone") / 2.0 / (pkg_total as f64).max(1.0),
            &format!("拉取 {}", t.package),
        );
        let r = plum_next::fetch::clone_or_update(
            &package_dir,
            &url,
            t.branch.as_deref(),
            &plum_next::fetch::FetchOpts::default(),
        );
        if r.is_ok() {
            clone_done = true;
            clone_used = Some(m.clone());
        }
        r
    })?;
    let used = if let Some(m) = clone_used {
        m
    } else {
        used
    };
    *last_mirror = Some(used.id.to_string());
    println!(
        "{} 配方包: {}  [{}] {}",
        action,
        t.rx(),
        used.label,
        t.branch
            .as_deref()
            .map(|b| format!("@{b}"))
            .unwrap_or_default()
    );
    if clone_done {
        progress::emit(
            "clone",
            base + stage_weight("clone") / (pkg_total as f64).max(1.0),
            &format!("拉取完成 {}", t.package),
        );
    }

    // ── 按配方安装 ──
    let recipe_path = match &t.recipe {
        Some(r) => {
            let p = package_dir.join(format!("{r}.recipe.yaml"));
            if !p.exists() {
                return Err(format!("配方文件不存在: {}", p.display()));
            }
            Some(p)
        }
        None => {
            let p = package_dir.join("recipe.yaml");
            if p.exists() {
                Some(p)
            } else {
                None
            }
        }
    };

    let mut updated = 0usize;
    if let Some(path) = recipe_path {
        let rx_label = t.rx();
        println!("  安装配方: {}", rx_label);
        let r = recipe::load(&path)?;
        // 声明校验：原版 `check_recipe_info`
        if let Some(declared) = &r.rx {
            if let Some(want) = &t.recipe {
                if declared != want {
                    return Err(format!(
                        "配方声明 '{declared}' 与订单里的 '{want}' 不一致"
                    ));
                }
            }
        }
        // 变量替换：配方 `args:` 的默认值 + 命令行选项（后者优先）
        let vars = recipe::merge_options(&r.args, &t.options);
        for entry in &r.download_files {
            let entry = recipe::substitute(entry, &vars);
            progress::emit(
                "download",
                base + stage_weight("download") / (pkg_total as f64).max(1.0),
                &format!("下载 {}", entry),
            );
            match install::download_entry(&entry, &package_dir) {
                Ok((name, true)) => println!("  下载: {name}"),
                Ok((name, false)) => println!("  已有: {name}"),
                Err(e) => eprintln!("  下载失败: {e}"),
            }
        }
        // `install_files:` 段存在就用它（**即使为空** —— 那是"什么都不装"，与原版一致）；
        // 段不存在才回退到"包里所有数据文件"。
        let files = if r.has_install_files {
            let patterns: Vec<String> = r
                .install_files
                .iter()
                .map(|p| recipe::substitute(p, &vars))
                .collect();
            install::pick(&package_dir, &patterns)
        } else {
            install::data_files(&package_dir)
        };
        updated += install::install(&package_dir, &args.rime_dir, &files)?;
        progress::emit(
            "install",
            base + stage_weight("install") / (pkg_total as f64).max(1.0),
            &format!("安装 {}", t.package),
        );
        for (file, body) in &r.patch_files {
            let file = recipe::substitute(file, &vars);
            let body = recipe::substitute(body, &vars);
            progress::emit(
                "patch",
                base + stage_weight("patch") / (pkg_total as f64).max(1.0),
                &format!("补丁 {}", file),
            );
            match recipe::apply_patch(&args.rime_dir, &file, &rx_label, &body) {
                Ok(()) => {
                    println!("  打补丁: {file}");
                    updated += 1;
                }
                Err(e) => eprintln!("  补丁失败 {file}: {e}"),
            }
        }
    } else {
        let files = install::data_files(&package_dir);
        updated += install::install(&package_dir, &args.rime_dir, &files)?;
        progress::emit(
            "install",
            base + stage_weight("install") / (pkg_total as f64).max(1.0),
            &format!("安装 {}", t.package),
        );
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::parse_selection;

    fn pkgs() -> Vec<String> {
        vec!["bopomofo".into(), "essay".into(), "prelude".into()]
    }

    #[test]
    fn empty_input_cancels() {
        assert!(parse_selection("", &pkgs()).is_empty());
        assert!(parse_selection("   ", &pkgs()).is_empty());
    }

    #[test]
    fn a_selects_all() {
        assert_eq!(parse_selection("a", &pkgs()), pkgs());
        assert_eq!(parse_selection("all", &pkgs()), pkgs());
    }

    #[test]
    fn numbers_pick_by_one_based_index() {
        assert_eq!(parse_selection("2", &pkgs()), vec!["essay".to_string()]);
        assert_eq!(
            parse_selection("1 3", &pkgs()),
            vec!["bopomofo".to_string(), "prelude".to_string()]
        );
    }

    #[test]
    fn out_of_range_and_duplicates_are_ignored() {
        assert_eq!(parse_selection("9", &pkgs()), Vec::<String>::new());
        assert_eq!(parse_selection("0", &pkgs()), Vec::<String>::new());
        assert_eq!(parse_selection("2 2", &pkgs()), vec!["essay".to_string()]);
        assert!(parse_selection("xyz", &pkgs()).is_empty());
    }
}
