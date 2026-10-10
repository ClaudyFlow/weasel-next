-- rime.lua —— rime-lua 的引导入口（librime-lua 只加载这个文件：用户目录优先，其次共享目录）
--
-- 作用（"lua 识别"）：把 lua 根目录加入搜索路径，使 lua_translator@<名字> 能直接找到
--   <共享目录>\lua\<名字>.lua 与 <用户目录>\lua\<名字>.lua（平铺，文件名即模块名，不需遍历文件夹）
local function add(dir)
  if dir and dir ~= "" then
    package.path = package.path .. ";" .. dir .. "/lua/?.lua"
  end
end
if rime_api and rime_api.get_user_data_dir then add(rime_api.get_user_data_dir()) end
if rime_api and rime_api.get_shared_data_dir then add(rime_api.get_shared_data_dir()) end