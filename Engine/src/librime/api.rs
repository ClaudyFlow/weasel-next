//! 读取 librime 版本化 API 表的安全前缀快照，并验证当前用途所需的入口。

use super::{RimeLibraryApi, raw};
use std::{
    mem::{offset_of, size_of},
    rc::Rc,
};

impl RimeLibraryApi {
    /// 按 `data_size` 安全读取 API 表前缀，并验证初始化或部署流程的必需入口。
    ///
    /// 部署模式只要求设置、最终化及部署入口；运行模式还要求会话、按键处理、
    /// 组合操作和状态读取及其配对释放入口。候选选择和翻页入口允许旧引擎缺省。
    /// `get_version` 不用于判定结构布局，ABI 边界以 `data_size` 为准。
    ///
    /// # Safety
    /// `source` 必须指向 librime 提供的、可读且在调用期间不可变的 API 表。其头部
    /// `data_size` 字段必须可读，且该字段声明的每个已读取字节都必须位于实际分配
    /// 区域内。函数指针必须符合对应 C ABI。
    pub(super) unsafe fn load(source: *const raw::RimeApi, deploy: bool) -> Result<Self, String> {
        if source.is_null() {
            return Err("librime returned a null API table".into());
        }
        // Read only the mandatory header before considering any function field.
        let data_size = unsafe { source.cast::<i32>().read() };
        let bytes = usize::try_from(data_size)
            .ok()
            .and_then(|size| size.checked_add(size_of::<i32>()))
            .ok_or_else(|| "invalid librime API data_size".to_owned())?;
        let mut snapshot: raw::RimeApi = unsafe { std::mem::zeroed() };
        macro_rules! field {
            ($name:ident) => {{
                // Never form a reference to the entire foreign table: older DLLs
                // may allocate only a prefix of the current header's structure.
                let end = offset_of!(raw::RimeApi, $name) + size_of_val(&snapshot.$name);
                if bytes >= end {
                    snapshot.$name = unsafe { std::ptr::addr_of!((*source).$name).read() };
                }
            }};
        }
        field!(setup);
        field!(initialize);
        field!(finalize);
        field!(deployer_initialize);
        field!(deploy);
        field!(create_session);
        field!(destroy_session);
        field!(process_key);
        field!(commit_composition);
        // 2026-11-07 修：辅助面板直拟上屏（点符号/剪贴板卡片）走 `commit_string`。
        // 这一项原来漏在白名单里 → 快照恒为 None → `mod.rs` 的 COMMIT_TEXT 臂静默走
        // `_` 分支（只 eprintln，Engine 常驻模式没有日志文件），用户看到的就是
        // 「点了符号没反应、候选窗还在」。函数表在 DLL 里明明有（vendor 头 319 行）。
        field!(commit_string);
        field!(clear_composition);
        field!(get_commit);
        field!(free_commit);
        field!(get_context);
        field!(get_current_schema);
        field!(get_input);
        field!(free_context);
        field!(get_status);
        field!(free_status);
        field!(get_option);
        field!(set_option);
        // 2026-10-29 补：漏装这一项时 `Session::select_schema` 恒返回 false
        // （函数指针是零值），引擎每个新会话都记一条"方案当前不可用，保留 librime 的选择"
        // —— 用户**在设置里选的当前方案从来没生效过**（真机日志 2026-10-29 09:47 复现）。
        field!(select_schema);
        field!(select_candidate_on_current_page);
        field!(change_page);
        macro_rules! required {
            ($($name:ident),+ $(,)?) => { $(
                if snapshot.$name.is_none() {
                    return Err(format!("librime API function is unavailable or outside data_size: {}", stringify!($name)));
                }
            )+ };
        }
        required!(setup, finalize);
        if deploy {
            required!(deployer_initialize, deploy);
        } else {
            required!(
                initialize,
                create_session,
                destroy_session,
                process_key,
                commit_composition,
                clear_composition,
                get_commit,
                free_commit,
                get_context,
                free_context,
                get_status,
                free_status,
                get_option,
                set_option
            );
        }
        // Candidate selection and change_page remain optional for older engines.
        Ok(Self {
            api: Rc::new(snapshot),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn setup(_: *mut raw::RimeTraits) {}
    unsafe extern "C" fn finalize() {}
    unsafe extern "C" fn deploy() -> i32 {
        1
    }
    unsafe extern "C" fn change_page(_: raw::RimeSessionId, _: i32) -> i32 {
        1
    }

    fn deploy_table() -> raw::RimeApi {
        let mut table: raw::RimeApi = unsafe { std::mem::zeroed() };
        table.data_size = super::super::struct_data_size::<raw::RimeApi>();
        table.setup = Some(setup);
        table.deployer_initialize = Some(setup);
        table.finalize = Some(finalize);
        table.deploy = Some(deploy);
        table.change_page = Some(change_page);
        table
    }

    #[test]
    fn older_table_does_not_expose_fields_beyond_its_version() {
        let mut table = deploy_table();
        table.data_size = (offset_of!(raw::RimeApi, change_page) - size_of::<i32>()) as i32;
        let api = unsafe { RimeLibraryApi::load(&table, true) }.unwrap();
        assert!(api.api.deploy.is_some());
        assert!(api.api.change_page.is_none());
    }

    #[test]
    fn future_table_versions_are_accepted() {
        let mut table = deploy_table();
        table.data_size += 128;
        let api = unsafe { RimeLibraryApi::load(&table, true) }.unwrap();
        assert!(api.api.change_page.is_some());
    }

    /// 直拟上屏依赖的 `commit_string` 必须真的从 DLL 函数表拷进快照。
    ///
    /// 2026-11-07 线上 bug：白名单漏了它，快照恒为 None，COMMIT_TEXT 臂静默失败
    /// （用户实测「点符号不上屏」），所以这里同时钉住"有则拷到、老表则留空"。
    #[test]
    fn commit_string_is_snapshotted_only_when_the_table_covers_it() {
        unsafe extern "C" fn commit_string(
            _: raw::RimeSessionId,
            _: *const std::os::raw::c_char,
        ) -> i32 {
            1
        }
        let mut table = deploy_table();
        table.commit_string = Some(commit_string);
        let api = unsafe { RimeLibraryApi::load(&table, true) }.unwrap();
        assert!(api.api.commit_string.is_some());
        // 老 DLL 的 data_size 不覆盖该字段时必须留 None，而不是越界读。
        table.data_size = (offset_of!(raw::RimeApi, commit_string) - size_of::<i32>()) as i32;
        let old = unsafe { RimeLibraryApi::load(&table, true) }.unwrap();
        assert!(old.api.commit_string.is_none());
    }

    #[test]
    fn missing_finalize_or_initialize_is_rejected_upfront() {
        let mut table = deploy_table();
        table.finalize = None;
        let error = unsafe { RimeLibraryApi::load(&table, true) }.err().unwrap();
        assert!(error.contains("finalize"));
        table.finalize = Some(finalize);
        let error = unsafe { RimeLibraryApi::load(&table, false) }
            .err()
            .unwrap();
        assert!(error.contains("initialize"));
    }

    #[test]
    fn header_only_and_negative_sizes_are_rejected_without_reading_fields() {
        for size in [-1i32, 0, 1] {
            assert!(unsafe { RimeLibraryApi::load((&size as *const i32).cast(), false) }.is_err());
        }
    }

    #[test]
    fn partial_function_pointer_is_not_available() {
        let mut table: raw::RimeApi = unsafe { std::mem::zeroed() };
        table.data_size = (offset_of!(raw::RimeApi, setup) + size_of_val(&table.setup)
            - size_of::<i32>()
            - 1) as i32;
        let error = unsafe { RimeLibraryApi::load(&table, false) }
            .err()
            .unwrap();
        assert!(error.contains("setup"));
    }
}
