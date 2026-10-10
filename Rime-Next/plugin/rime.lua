-- rime.lua —— rime-lua 引导入口（librime-lua 只加载这个文件：用户目录优先，其次共享目录）
--
-- 布局约定：插件放**外壳文件夹**里 —— plugin\<名字>\<名字>.lua（文件名即模块名）
-- 本引导做两件事：
--   1) 把 plugin\<名字>\ 加入 package.path（lua_translator@<名字> 可直接 find 到）
--   2) 用全路径 dofile 载入插件并把组件注册成**全局**（供 @*名字 挂载）

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

-- 插件清单（外壳文件夹名 = 模块名）
local PLUGINS = { "input_staticstic" }

for _, name in ipairs(PLUGINS) do
  local dirs = {}
  if user_dir() then dirs[#dirs + 1] = user_dir() .. "/plugin/" .. name end
  if shared_dir() then dirs[#dirs + 1] = shared_dir() .. "/plugin/" .. name end

  local loaded = false
  for _, dir in ipairs(dirs) do
    package.path = package.path .. ";" .. dir .. "/?.lua"
    boot_log("搜索路径: " .. dir .. "/?.lua")
    local file = dir .. "/" .. name .. ".lua"
    local ok, result = pcall(dofile, file)
    if ok and type(result) == "table" then
      boot_log("已载入: " .. file)
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