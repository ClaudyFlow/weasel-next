-- input_staticstic.lua —— 打字统计（Lua 版，rime-lua 原版机制）
--
-- 位置：<用户目录>/lua/input_staticstic.lua（平铺在 lua 根目录，文件名即模块名）
--      出厂副本：<程序目录>/Rime-Next/lua/input_staticstic.lua
-- 入口：<用户目录>/rime.lua（或出厂 <共享目录>/rime.lua）把 lua 根目录加入 package.path
-- 挂载（方案 *.custom.yaml）：
--   patch:
--     "engine/processors/@before 0": lua_processor@input_staticstic
--     "engine/translators/@next":    lua_translator@input_staticstic
--
-- 命令：tj / rtj / ztj / ytj / ntj / qtj / tjq / tjs（不带斜杠、单个 j）
-- 存储：优先 LevelDB（rime-lua 的 userdb 模块 → <用户目录>/userdb/input_stat.userdb），
--       不可用时退回 <用户目录>/lua/input_staticstic.txt（纯文本，键=值）。
--
-- 口径（与 Rust/C++ 版一致）：
--   词数=上屏次数；字数=上屏字符数；按键=输入过程中的按键（退格各计一次）
--   有效按键=按键-2*退避；用时=相邻按键间隔之和（>5s 视为停顿不计）
--   键准=有效按键/按键；均速=字数/用时（字/分）；极速=每满 60s 用时结算一次，取最大且不低于均速
--   击键=按键/用时（键/秒）；码长=按键/字数；理论码长=有效按键/字数
--   理论值 = 实际值 x 键准（理论极速/理论均速同理）
--   数字千分位用**空格**分隔（不用逗号）；时区标签写作 "UTC +08:00"

local M = {}

-- 自证：模块被引擎 require 到 
local function mod_log(msg)
  local dir = (rime_api and rime_api.get_user_data_dir and rime_api.get_user_data_dir()) or "."
  local f = io.open(dir .. "/plugin/_module.log", "a")
  if f then f:write(os.date("%Y-%m-%d %H:%M:%S") .. "  " .. msg .. "\n"); f:close() end
end
mod_log("模块已被 require")
-- 诊断：确认引擎真的加载并调用了本模块（写入 plugin/_input_staticstic.log）
local function diag(msg)
  local dir = (rime_api and rime_api.get_user_data_dir and rime_api.get_user_data_dir()) or "."
  local f = io.open(dir .. "/plugin/_input_staticstic.log", "a")
  if f then
    f:write(os.date("%Y-%m-%d %H:%M:%S") .. "  " .. msg .. "\n")
    f:close()
  end
end

local IDLE_MS = 5000
local SPEED_WINDOW_MS = 60000
local SCRIPT_VERSION = "₂₀₂₆1010・A"
local MOTTO = "知识从我而来，问题到我为止。"
local FALLBACK_NAME = "input_staticstic.txt"

----------------------------------------------------------------------
-- 环境
----------------------------------------------------------------------

local function user_dir()
  if rime_api and rime_api.get_user_data_dir then
    return rime_api.get_user_data_dir()
  end
  return "."
end

local function join(a, b)
  return a .. "/" .. b
end

local device_name = "本机"

local function read_device()
  local f = io.open(join(user_dir(), "installation.yaml"), "r")
  if not f then return end
  for line in f:lines() do
    local v = line:match("^%s*installation_id:%s*[\"']?([%w%-_]+)[\"']?%s*$")
    if v then device_name = v end
  end
  f:close()
end

----------------------------------------------------------------------
-- 数字格式：千分位用空格（逗号换空格）
----------------------------------------------------------------------

local function num(value)
  local n = math.floor(tonumber(value) or 0)
  local sign = ""
  if n < 0 then sign = "-"; n = -n end
  local s = tostring(n)
  local out = s:reverse():gsub("(%d%d%d)", "%1 "):reverse()
  out = out:gsub("^%s+", ""):gsub("%s+$", "")
  return sign .. out
end

----------------------------------------------------------------------
-- 存储：LevelDB（userdb）优先，退回文本文件
----------------------------------------------------------------------

local store = { kind = "none", handle = nil, path = nil }

local function try_open_userdb()
  local ok, userdb = pcall(require, "userdb")
  if not ok or type(userdb) ~= "table" or type(userdb.open) ~= "function" then
    return false
  end
  local ok2, db = pcall(userdb.open, "input_stat", "userdb")
  if not ok2 or db == nil then return false end
  store.kind = "userdb"
  store.handle = db
  return true
end

local function try_open_text()
  local p = join(join(user_dir(), "lua"), FALLBACK_NAME)
  local f = io.open(p, "a")
  if not f then return false end
  f:close()
  store.kind = "text"
  store.path = p
  return true
end

local function open_store()
  if try_open_userdb() then return end
  if try_open_text() then return end
  store.kind = "none"
end

local function db_get(key)
  if store.kind == "userdb" then
    local ok, value = pcall(function() return store.handle:fetch(key) end)
    if ok then return value end
    return nil
  elseif store.kind == "text" then
    local f = io.open(store.path, "r")
    if not f then return nil end
    local found = nil
    for line in f:lines() do
      local k, v = line:match("^(.-)\t(.*)$")
      if k == key then found = v end
    end
    f:close()
    return found
  end
  return nil
end

local function db_set(key, value)
  if store.kind == "userdb" then
    pcall(function() store.handle:update(key, value) end)
  elseif store.kind == "text" then
    local rows, seen = {}, false
    local f = io.open(store.path, "r")
    if f then
      for line in f:lines() do
        local k = line:match("^(.-)\t")
        if k == key then rows[#rows + 1] = key .. "\t" .. value; seen = true
        else rows[#rows + 1] = line end
      end
      f:close()
    end
    if not seen then rows[#rows + 1] = key .. "\t" .. value end
    local w = io.open(store.path, "w")
    if w then
      for _, line in ipairs(rows) do w:write(line, "\n") end
      w:close()
    end
  end
end

----------------------------------------------------------------------
-- 数据：按天一行，序列化成 "k=v;k=v"
----------------------------------------------------------------------

local days = {}

local function blank()
  return {
    commits = 0, chars = 0, keys = 0, backs = 0, dur_ms = 0,
    win_chars = 0, win_ms = 0, best = 0,
  }
end

local FIELDS = { "commits", "chars", "keys", "backs", "dur_ms", "win_chars", "win_ms", "best" }

local function decode(text)
  if type(text) ~= "string" then return nil end
  local d = blank()
  for k, v in text:gmatch("([%a_]+)=(%-?%d+%.?%d*)") do
    if d[k] ~= nil then d[k] = tonumber(v) or d[k] end
  end
  return d
end

local function encode(d)
  local parts = {}
  for _, name in ipairs(FIELDS) do
    parts[#parts + 1] = name .. "=" .. tostring(d[name] or 0)
  end
  return table.concat(parts, ";")
end

local function today()
  return os.date("%Y-%m-%d")
end

local function ensure_ready()
  if store.kind == "none" then
    read_device()
    open_store()
  end
end

local function day(key)
  ensure_ready()
  local d = days[key]
  if not d then
    d = decode(db_get("day:" .. key)) or blank()
    days[key] = d
  end
  return d
end

local function flush(key)
  db_set("day:" .. key, encode(day(key)))
end

----------------------------------------------------------------------
-- 汇总
----------------------------------------------------------------------

local function merge(keys)
  local t = blank()
  for _, k in ipairs(keys) do
    local d = day(k)
    for _, name in ipairs(FIELDS) do
      if name ~= "win_chars" and name ~= "win_ms" then
        t[name] = t[name] + (d[name] or 0)
      end
    end
    if (d.best or 0) > t.best then t.best = d.best end
  end
  return t
end

local function range_keys(range)
  local out = {}
  local now = os.date("*t")
  local function push(y, m, d)
    local key = string.format("%04d-%02d-%02d", y, m, d)
    if days[key] or db_get("day:" .. key) then out[#out + 1] = key end
  end
  if range == "today" then
    push(now.year, now.month, now.day)
  elseif range == "week" then
    local wd = (now.wday == 1) and 7 or (now.wday - 1)
    for i = 0, wd - 1 do push(now.year, now.month, now.day - i) end
  elseif range == "month" then
    for i = 1, now.day do push(now.year, now.month, i) end
  elseif range == "year" then
    for m = 1, now.month do
      local last = (m == now.month) and now.day or 31
      for d = 1, last do push(now.year, m, d) end
    end
  else
    for k in pairs(days) do out[#out + 1] = k end
    if store.kind == "text" and store.path then
      local f = io.open(store.path, "r")
      if f then
        for line in f:lines() do
          local k = line:match("^day:(%d%d%d%d%-%d%d%-%d%d)\t")
          if k then out[#out + 1] = k end
        end
        f:close()
      end
    end
  end
  return out
end

----------------------------------------------------------------------
-- 报表
----------------------------------------------------------------------

local function rate(chars, ms)
  if ms <= 0 then return 0 end
  return chars * 60000.0 / ms
end

local function fmt1(v) return string.format("%.1f", v) end

-- 时区标签：UTC 与偏移之间留一个空格
local function tz_label()
  local z = os.date("%z")
  if z and #z == 5 then
    return "UTC " .. z:sub(1, 3) .. ":" .. z:sub(4, 5)
  end
  return "UTC +08:00"
end

local function report(range, label)
  local t = merge(range_keys(range))
  local acc = (t.keys > 0) and ((t.keys - 2 * t.backs) / t.keys) or 1
  local avg = rate(t.chars, t.dur_ms)
  local best = math.max(t.best or 0, avg)
  local kps = (t.dur_ms > 0) and (t.keys * 1000.0 / t.dur_ms) or 0
  local code_len = (t.chars > 0) and (t.keys / t.chars) or 0
  local theo_len = (t.chars > 0) and ((t.keys - 2 * t.backs) / t.chars) or 0
  local theo_kps = kps * acc
  local theo_avg = avg * acc
  local theo_best = best * acc
  local secs = math.floor(t.dur_ms / 1000)
  local words = (t.commits > 0) and t.commits or 1
  local dur = string.format("%d时%d分%d秒", math.floor(secs / 3600), math.floor(secs % 3600 / 60), secs % 60)
  local stamp = os.date("%Y-%m-%d %H:%M:%S")

  return table.concat({
    string.format("%s  %s  %s  %s", label, stamp, tz_label(), device_name),
    string.format("上屏 %s 次 输入 %s 字 用时 %s", num(t.commits), num(t.chars), dur),
    string.format("实际极速 %s 击键 %s 码长 %s", fmt1(best), fmt1(kps), fmt1(code_len)),
    string.format("实际均速 %s 击键 %s 码长 %s", fmt1(avg), fmt1(kps), fmt1(code_len)),
    string.format("理论极速 %s 击键 %s 码长 %s", fmt1(theo_best), fmt1(theo_kps), fmt1(theo_len)),
    string.format("理论均速 %s 击键 %s 码长 %s", fmt1(theo_avg), fmt1(theo_kps), fmt1(theo_len)),
    string.format("误触 %s 次 误触率 %.0f%% 键准 %.0f%%", num(t.backs), (1 - acc) * 100, acc * 100),
    string.format("平均词长 %.2f", t.chars / words),
    MOTTO,
    "脚本：🐐🪶❄️" .. SCRIPT_VERSION,
  }, "\n")
end

local RANGES = {
  { key = "today", label = "今日", letter = "r" },
  { key = "week", label = "本周", letter = "z" },
  { key = "month", label = "本月", letter = "y" },
  { key = "year", label = "本年", letter = "n" },
  { key = "all", label = "累计", letter = "q" },
}

local function range_of(letter)
  if letter == "" or letter == "q" then return "all" end
  if letter == "r" then return "today" end
  if letter == "z" then return "week" end
  if letter == "y" then return "month" end
  if letter == "n" then return "year" end
  return nil
end

local function label_of(range)
  for _, r in ipairs(RANGES) do
    if r.key == range then return r.label end
  end
  return range
end

----------------------------------------------------------------------
-- 生命周期 + 事件
----------------------------------------------------------------------

local session = { last = 0 }

local function now_ms()
  return math.floor(os.time() * 1000)
end

local function count_key(d)
  d.keys = d.keys + 1
  local t = now_ms()
  if session.last > 0 then
    local gap = t - session.last
    if gap > 0 and gap <= IDLE_MS then
      d.dur_ms = d.dur_ms + gap
      d.win_ms = d.win_ms + gap
    end
  end
  session.last = t
end

local function on_commit(d, text)
  d.commits = d.commits + 1
  d.chars = d.chars + (utf8.len(text) or #text)
end

local function init_impl(env)
  read_device()
  open_store()
  diag("init ok, store=" .. tostring(store.kind))
  mod_log("init 被调用")
  if env and env.engine and env.engine.context then
    env.commit_connection = env.engine.context.commit_notifier:connect(function(ctx)
      local text = ctx:get_commit_text() or ""
      if text ~= "" then
        local key = today()
        on_commit(day(key), text)
        flush(key)
      end
    end)
  end
end

local function fini_impl(env)
  if env and env.commit_connection then
    env.commit_connection:disconnect()
    env.commit_connection = nil
  end
  for key in pairs(days) do flush(key) end
end

local function processor_impl(key, env)
  if key:release() then return 2 end
  local code = key.keycode
  local d = day(today())
  if code == 0xFF08 then          -- BackSpace：算一次按键，并记一次误触
    d.backs = d.backs + 1
    count_key(d)
  elseif code and code >= 0x20 and code <= 0x7E then
    count_key(d)
  end
  flush(today())
  return 2                        -- kNoop：不干预输入
end

local function translator_impl(input, seg, env)
  diag("translator input=" .. tostring(input))
  local letter = input:match("^([rzynq]?)tj[qs]?$")
  if letter == nil then return end
  diag("命中命令 input=" .. tostring(input))
  local range = range_of(letter)
  if not range then return end
  if letter == "" then
    for _, r in ipairs(RANGES) do
      yield(Candidate("statistics", seg.start, seg._end, report(r.key, r.label), r.label .. "统计"))
    end
  else
    local label = label_of(range)
    yield(Candidate("statistics", seg.start, seg._end, report(range, label), label .. "统计"))
  end
end

M.init = init_impl
M.fini = fini_impl
M.processor = { func = processor_impl }
M.translator = { func = translator_impl }

return M
