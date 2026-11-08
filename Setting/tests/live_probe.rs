//! 临时诊断测试（不是正式测试）：用**线上真实目录**跑一遍设置界面的加载链路，
//! 看「常规」页为什么一条字段都没有。跑完可删。
use std::path::PathBuf;
use weasel_common::process::RuntimePaths;
use weasel_settings::{catalog, config, form};

fn live_paths() -> RuntimePaths {
    RuntimePaths {
        executable_directory: PathBuf::from(r"C:\Program Files\Rime-Next\Weasel-Next-0.18.5"),
        user_data: PathBuf::from(r"C:\Users\CloudyFlow\AppData\Roaming\Rime-Next"),
        logs: PathBuf::from(r"C:\Users\CloudyFlow\AppData\Local\Rime-Next\Logs"),
    }
}

#[test]
fn probe_live_catalog() {
    let paths = live_paths();
    println!("==== Document::load ====");
    let doc = match config::Document::load(&paths) {
        Ok(d) => {
            println!("OK  base 顶层键 = {:?}", d.base.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()));
            println!("    patch 顶层键 = {:?}", d.patch.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()));
            d
        }
        Err(e) => {
            println!("ERR {e}");
            return;
        }
    };

    println!("==== catalog::load ====");
    let cat = match catalog::load(&paths, &doc) {
        Ok(c) => {
            println!("OK  sections = {}", c.sections.len());
            println!("    notices = {:?}", c.notices);
            for (i, s) in c.sections.iter().enumerate() {
                println!("    [{i}] title={:?} mount={:?}", s.title, s.mount);
            }
            c
        }
        Err(e) => {
            println!("ERR {e}");
            return;
        }
    };

    println!("==== form::fields 逐段 ====");
    for (i, s) in cat.sections.iter().enumerate() {
        match form::fields(s, &doc) {
            Ok(f) => {
                let keys: Vec<&str> = f.iter().take(6).map(|x| x.key.as_str()).collect();
                println!("    [{i}] {:?} -> {} 条字段  前几个={:?}", s.title, f.len(), keys);
            }
            Err(e) => println!("    [{i}] {:?} -> ERR {e}", s.title),
        }
    }
}
