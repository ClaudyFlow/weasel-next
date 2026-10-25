-- rime.lua —— rime-lua 引导入口（librime-lua 只加载这个文件：用户目录优先，其次共享目录）
-- [bootstrap-v2] 把程序文件夹的 plugin 也加进 package.path（2026-10-23）
--   ↑ 这一行是**版本标记**：deploy-install.ps1 靠它判断"用户目录那份是不是旧版、要不要升级"。
--     别删；升级引导逻辑时把 v2 改成 v3 并同步脚本里的判定。
--
-- 布局约定：插件放**外壳文件夹**里 —— plugin\<名字>\<名字>.lua（文件名即模块名）
-- 本引导做三件事：
--   1) 把 plugin\ 与 plugin\<名字>\ 加入 package.path（@*名字.函数 的 require 才找得到）
--   2) 用全路径 dofile 载入插件并把组件注册成**全局**（供 @*名字 挂载）
--   3) **用户目录与程序文件夹都要进 package.path** —— 安装包把插件放在程序文件夹，
--      只加用户目录的话，新装机器上 require("yuhao.…") 找不到文件，宇浩的
--      lua_processor@*yuhao.xxx 组件会被静默丢弃（2026-10-23 别人反馈「自分割没了」就是这个）。

local function user_dir()
  if rime_api and rime_api.get_user_data_dir then return rime_api.get_user_data_dir() end
  return "."
end
local function shared_dir()
  if rime_api and rime_api.get_shared_data_dir then return rime_api.get_shared_data_dir() end
  return nil
end

local LOG = user_dir() .. "/plugin/_boot.log"
local function boot_log(msg)
  local f = io.open(LOG, "a")
  if f then f:write(os.date("%Y-%m-%d %H:%M:%S") .. "  " .. msg .. "\n"); f:close() end
end

boot_log("=== rime.lua 开始加载 ===")

-- 子模块搜索路径：让 *yuhao.xxx 这类 require 能找到 plugin\yuhao\xxx.lua。
-- 用户目录在前（可覆盖），程序文件夹在后（安装包自带的那份）。
local function add_path(dir)
  if not dir then return end
  package.path = package.path .. ";" .. dir .. "/?.lua"
  boot_log("搜索路径: " .. dir .. "/?.lua")
end

add_path(user_dir() .. "/plugin")
add_path(shared_dir() and (shared_dir() .. "/plugin") or nil)

-- 插件清单（外壳文件夹名 = 模块名）
local PLUGINS = { "input_statistics" }

for _, name in ipairs(PLUGINS) do
  local dirs = {}
  if user_dir() then dirs[#dirs + 1] = user_dir() .. "/plugin/" .. name end
  if shared_dir() then dirs[#dirs + 1] = shared_dir() .. "/plugin/" .. name end

  local loaded = false
  for _, dir in ipairs(dirs) do
    add_path(dir)
    local file = dir .. "/" .. name .. ".lua"
    local ok, result = pcall(dofile, file)
    if ok and type(result) == "table" then
      boot_log("已载入: " .. file)
      -- ⚠️ 这里的名字故意保持原样（包括 `input_staticstic` 这个拼写错误）：
      -- 条件永远不成立 ⇒ 下面几个全局从来没被赋过值。改掉它会引入一个**行为变化**，
      -- 而这次只想修路径。要修另开一次改动 + 单独验证。
      if name == "input_staticstic" then
        input_staticstic_processor = result.processor
        input_staticstic_translator = result.translator
        boot_log("全局 input_staticstic_processor = " .. type(input_staticstic_processor))
        boot_log("全局 input_staticstic_translator = " .. type(input_staticstic_translator))
        if type(result.init) == "function" then
          local inner_init = result.init
          init = function(env) inner_init(env) end
          boot_log("全局 init 已注册 ✓")
        end
        if type(result.fini) == "function" then
          local inner_fini = result.fini
          fini = function(env) inner_fini(env) end
        end
      end
      loaded = true
      break
    else
      boot_log("载入失败 " .. file .. " => " .. tostring(result))
    end
  end
  if not loaded then boot_log("插件 " .. name .. " 未载入 ✗") end
end

-- ── 虎整句（TigerClaw）注册 ─────────────────────────────────────────
-- 模块在 plugin\tiger\（8 个 lua 平铺），用户目录与程序文件夹都找。
add_path(user_dir() .. "/plugin/tiger")
add_path(shared_dir() and (shared_dir() .. "/plugin/tiger") or nil)
local ok_tiger, tiger_sentence = pcall(require, "tiger_sentence")
if not ok_tiger then
  boot_log("虎整句 require 失败: " .. tostring(tiger_sentence))
else
  tiger_sentence_processor = tiger_sentence.processor_component
  tiger_sentence_translator = tiger_sentence.translator
  tiger_sentence_ascii = tiger_sentence.ascii_component
  tiger_sentence_buffer_filter = tiger_sentence.buffer_filter
  boot_log("虎整句已注册 tiger_sentence_translator = " .. type(tiger_sentence_translator))
end
