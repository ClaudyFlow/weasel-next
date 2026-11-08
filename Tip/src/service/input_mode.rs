//! 通过宿主 TSF 线程管理器的私有 compartment 保存已确认的输入模式。
//!
//! 该值只表达输入模式，不保存连接或组合状态；服务停用或析构时不清除此宿主值，
//! 也不使用跨进程全局变量。
use super::*;
use crate::bindings::{GUID_WEASEL_INPUT_MODE, ITfCompartmentMgr, VariantClear};

/// 读取私有 compartment；仅接受 `VT_I4` 的 `0`/`1`，其他值视为未知。
fn read(manager: &ITfThreadMgr) -> Result<Option<bool>> {
    let compartments: ITfCompartmentMgr = manager.cast()?;
    let compartment = unsafe { compartments.GetCompartment(&GUID_WEASEL_INPUT_MODE)? };
    let mut value = unsafe { compartment.GetValue()? };
    let mode = unsafe {
        let inner = &value.Anonymous.Anonymous;
        if inner.vt == VARTYPE(VT_I4 as u16) {
            match inner.Anonymous.lVal {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            }
        } else {
            None
        }
    };
    unsafe {
        let _ = VariantClear(&mut value);
    }
    Ok(mode)
}

/// 把 ASCII 模式写入宿主线程管理器的私有 compartment。
///
/// `tid` 必须是当前服务向 TSF 注册时取得的客户端 ID；传给 COM 的 VARIANT 按 ABI 布局构造。
fn write(manager: &ITfThreadMgr, tid: TfClientId, ascii: bool) -> Result<()> {
    let compartments: ITfCompartmentMgr = manager.cast()?;
    let compartment = unsafe { compartments.GetCompartment(&GUID_WEASEL_INPUT_MODE)? };
    let value = VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VARTYPE(VT_I4 as u16),
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    lVal: i32::from(ascii),
                },
            }),
        },
    };
    unsafe { compartment.SetValue(tid, &value).ok() }
}

/// 系统输入指示器（任务栏那个「中/英」）读的**标准**转换模式隔间。
///
/// 值取自 Windows SDK `msctf.h` 的 `GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION`
/// = `{ccf05dd8-4a87-11d7-a6e2-00065b84435c}`。原版小狼毫就是靠写它让任务栏图标
/// 跟着中英切换变的（`WeaselTSF::_UpdateLanguageBar` → `_SetCompartmentDWORD`）。
/// 我们之前只写私有隔间 `GUID_WEASEL_INPUT_MODE`，系统那边永远不知道模式变了
/// —— 这就是"按 Shift 任务栏没反应"的根因。
const GUID_COMPARTMENT_CONVERSION: GUID = GUID::from_u128(0xccf05dd8_4a87_11d7_a6e2_00065b84435c);

/// `TF_CONVERSIONMODE_NATIVE`：置位 = 本语言（中文）输入，清位 = 英文。
const TF_CONVERSIONMODE_NATIVE: i32 = 0x0001;

/// 把中/英写进标准转换模式隔间，**只改 NATIVE 位**、保留其它位（全角等）。
///
/// 与 `WeaselTSF::_UpdateLanguageBar` 等价：先读现有 flags，再按 ascii 置/清
/// `TF_CONVERSIONMODE_NATIVE`，最后整值写回。写入失败只记诊断，绝不阻断输入。
fn write_conversion_mode(manager: &ITfThreadMgr, tid: TfClientId, ascii: bool) -> Result<()> {
    let compartments: ITfCompartmentMgr = manager.cast()?;
    let compartment = unsafe { compartments.GetCompartment(&GUID_COMPARTMENT_CONVERSION)? };
    let mut flags: i32 = 0;
    if let Ok(mut current) = unsafe { compartment.GetValue() } {
        unsafe {
            let inner = &current.Anonymous.Anonymous;
            if inner.vt == VARTYPE(VT_I4 as u16) {
                flags = inner.Anonymous.lVal;
            }
            let _ = VariantClear(&mut current);
        }
    }
    if ascii {
        flags &= !TF_CONVERSIONMODE_NATIVE;
    } else {
        flags |= TF_CONVERSIONMODE_NATIVE;
    }
    let value = VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VARTYPE(VT_I4 as u16),
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: flags },
            }),
        },
    };
    unsafe { compartment.SetValue(tid, &value).ok() }
}

impl TextService {
    /// 激活时读取宿主保存的输入模式，并同步到 RPC 工作线程。
    ///
    /// TSF 读取失败只报告诊断并记为未知，不阻止文本服务继续工作。
    pub(super) fn load_input_mode(&self) -> Result<()> {
        let manager = self.lock(&self.thread_mgr)?.clone();
        let mode = match manager.as_ref().map(read).transpose() {
            Ok(mode) => mode.flatten(),
            Err(error) => {
                crate::rpc_diagnostics::report("input-mode-read", None, error);
                None
            }
        };
        // 激活时也把标准转换模式隔间刷一次：切到这个输入法时任务栏「中/英」立刻对上，
        // 不等第一次按键（原版小狼毫在 `OnSetThreadFocus` 里做同样的事）。
        if let (Some(manager), Some(ascii)) = (manager.as_ref(), mode) {
            let tid = *self.lock(&self.keystroke_client_id)?;
            if let Some(tid) = tid {
                if let Err(error) = write_conversion_mode(manager, tid, ascii) {
                    crate::rpc_diagnostics::report("input-mode-conversion", None, error);
                }
            }
        }
        self.lock(&self.rpc)?.remember_mode(mode);
        Ok(())
    }

    /// 记住引擎确认的新模式，并尽力持久化到宿主 compartment。
    ///
    /// 调用 TSF 前先释放服务状态锁，以免宿主回调重入时发生锁死；写入失败不应禁用输入。
    pub(super) fn remember_input_mode(&self, ascii: bool) -> Result<()> {
        if self.lock(&self.rpc)?.remembered_mode() == Some(ascii) {
            return Ok(());
        }
        // No service locks are held while calling into TSF.
        let manager = self.lock(&self.thread_mgr)?.clone();
        let tid = *self.lock(&self.keystroke_client_id)?;
        if let (Some(manager), Some(tid)) = (manager, tid) {
            if let Err(error) = write(&manager, tid, ascii) {
                // Remembering a preference must never disable host input.
                crate::rpc_diagnostics::report("input-mode-write", None, error);
            }
            // 再写**标准**隔间 —— 任务栏/语言栏那个「中/英」指示器读的是它。
            if let Err(error) = write_conversion_mode(&manager, tid, ascii) {
                crate::rpc_diagnostics::report("input-mode-conversion", None, error);
            }
        }
        self.lock(&self.rpc)?.remember_mode(Some(ascii));
        Ok(())
    }
}
