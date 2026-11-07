-- rime.lua —— rime-lua 引导入口（librime-next 只加载 plugin/rime.lua 这一个文件：
-- 共享目录先、用户目录后，用户那份可覆盖官方注册）
-- [bootstrap-v3] 2026-11-07 恒等式布局：模块一律 plugin/<id 按 '.' 分段>/<完整 id>.lua，
--   plugin 根下只允许 rime.lua 这一个单文件，lua/ 目录与一切扁平形态已废除。
--   搜索由引擎的恒等式 searcher 强制（替换了 package.searchers[2]），本文件
--   **不再写 package.path** —— 写了也解析不到。旧 v2 标记作废，deploy 脚本
--   若还认 v2 需同步升级。

local function user_dir()
  if rime_api and rime_api.get_user_data_dir then return rime_api.get_user_data_dir() end
  return "."
end

local LOG = user_dir() .. "/plugin/_boot.log"
local function boot_log(msg)
  local f = io.open(LOG, "a")
  if f then f:write(os.date("%Y-%m-%d %H:%M:%S") .. "  " .. msg .. "\n"); f:close() end
end

boot_log("=== rime.lua 开始加载（bootstrap-v3 恒等式布局） ===")

-- 虎整句（TigerClaw）：require 走恒等式 searcher（plugin/tiger_sentence/tiger_sentence.lua），
-- 注册成全局供 @*tiger_sentence_* 组件挂载
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

-- input_statistics（plugin/input_statistics/input_statistics.lua）
local ok_is, input_statistics = pcall(require, "input_statistics")
if ok_is then
  boot_log("input_statistics 已载入 = " .. type(input_statistics))
else
  boot_log("input_statistics require 失败: " .. tostring(input_statistics))
end
