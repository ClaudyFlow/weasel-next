//! Native file properties shared by EXEs and the TIP DLL.
pub fn embed() {
    let package = std::env::var("CARGO_PKG_NAME").expect("Cargo package name");
    let (description, filename, dll) = match package.as_str() {
        "Weasel-Next-Server" => ("Weasel-Next Broker", "Weasel-Next-Server.exe", false),
        "Weasel-Next-Engine" => ("Weasel-Next Rime Server", "Weasel-Next-Engine.exe", false),
        "Weasel-Next-Renderer" => ("Weasel-Next Candidate Renderer", "Weasel-Next-Renderer.exe", false),
        "Weasel-Next-Setting" => ("Weasel-Next Settings", "Weasel-Next-Setting.exe", false),
        "weasel-tip" => ("Weasel-Next Text Input Processor", "Weasel-Next.dll", true),
        "weasel-theme-official" => ("Weasel-Next official Theme", "weasel-next-theme.dll", true),
        _ => panic!("missing Windows file description for {package}"),
    };
    // 向上搜索 workspace 根部的 `build_support`，不依赖 crate 所在目录深度
    // （主题 crate 可能位于 `theme/` 或 `theme/api` 等不同层级）。
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = {
        let mut dir = manifest_dir.as_path();
        loop {
            if dir.join("build_support/version.rc").exists() {
                break dir.to_path_buf();
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break manifest_dir.join(".."),
            }
        }
    };
    let resource = root.join("build_support/version.rc");
    for path in [&resource, &root.join("build_support/version.rs")] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed=Cargo.toml");
    let version = std::env::var("CARGO_PKG_VERSION").expect("Cargo package version");
    let numeric: Vec<_> = ["MAJOR", "MINOR", "PATCH"]
        .map(|part| {
            std::env::var(format!("CARGO_PKG_VERSION_{part}"))
                .expect("Cargo version component")
                .parse::<u16>()
                .expect("Windows version component must fit in 16 bits")
        })
        .into_iter()
        .collect();
    let quote = |name: &str, value: &str| {
        format!(
            "{name}=\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        )
    };
    let definitions = [
        format!(
            "WEASEL_FILE_VERSION={},{},{},0",
            numeric[0], numeric[1], numeric[2]
        ),
        format!("WEASEL_FILE_TYPE={}", if dll { 2 } else { 1 }),
        quote("WEASEL_FILE_DESCRIPTION", description),
        quote("WEASEL_VERSION_STRING", &version),
        quote("WEASEL_INTERNAL_NAME", &package),
        quote("WEASEL_ORIGINAL_FILENAME", filename),
    ];
    let result = if dll {
        embed_resource::compile_for_cdylib(resource, &definitions)
    } else {
        embed_resource::compile_for(resource, [package.as_str()], &definitions)
    };
    result
        .manifest_required()
        .expect("failed to embed Windows VERSIONINFO");
}
