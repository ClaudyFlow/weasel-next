-- Scalar-safe UTF-8 inventory, shared by all sessions in this Lua instance.
local M = { revision = 0, mode = "common" }
M.options = { tiger_sentence_characters_common = "common", tiger_sentence_characters_full = "full" }
M.path = "options/tiger_sentence_character_set"
function M.valid(value) return value == "common" or value == "full" end
local function scalars(text, visit)
    local i = 1
    while i <= #text do
        local first, n, value = text:byte(i), 1, text:byte(i)
        if first >= 0xc2 and first <= 0xdf then n, value = 2, first - 0xc0
        elseif first >= 0xe0 and first <= 0xef then n, value = 3, first - 0xe0
        elseif first >= 0xf0 and first <= 0xf4 then n, value = 4, first - 0xf0
        elseif first >= 0x80 then error("Invalid UTF-8 inventory") end
        for j = 1, n - 1 do
            local b = text:byte(i + j)
            if not b or b < 0x80 or b > 0xbf then error("Invalid UTF-8 inventory") end
            value = value * 64 + b - 0x80
        end
        if (n == 2 and value < 0x80) or (n == 3 and value < 0x800) or
            (n == 4 and value < 0x10000) or value > 0x10ffff or
            (value >= 0xd800 and value <= 0xdfff) then error("Invalid UTF-8 inventory") end
        visit(value)
        i = i + n
    end
end
function M.han(c)
    return c == 0x3007 or (c >= 0x3400 and c <= 0x4dbf) or
        (c >= 0x4e00 and c <= 0x9fff) or (c >= 0xf900 and c <= 0xfaff) or
        (c >= 0x20000 and c <= 0x2ee5f) or (c >= 0x2f800 and c <= 0x2fa1f) or
        (c >= 0x30000 and c <= 0x3347f)
end
function M.parse(text)
    local result = {}
    scalars(text, function(c)
        local white = (c >= 9 and c <= 13) or c == 32 or c == 0x85 or c == 0xa0 or c == 0x1680 or
            (c >= 0x2000 and c <= 0x200a) or c == 0x2028 or c == 0x2029 or c == 0x202f or c == 0x205f or c == 0x3000
        if not white and c ~= 0xfeff then result[c] = true end
    end)
    return result
end
local inventory = M.parse(require("tiger_sentence_common_data"))
local previous
function M.reload(path)
    local file = io.open(path, "rb")
    if not file then return false end
    local text = file:read("*a"); file:close()
    if not text or text == previous then return false end
    local ok, next_inventory = pcall(M.parse, text)
    if not ok then return false end
    local changed = false
    for c in pairs(inventory) do if not next_inventory[c] then changed = true; break end end
    for c in pairs(next_inventory) do if not inventory[c] then changed = true; break end end
    previous, inventory = text, next_inventory
    if not changed then return false end
    M.revision = M.revision + 1
    return true
end
function M.allows(text, ordinary)
    if M.mode == "full" then return true end
    local count, allowed = 0, true
    scalars(text, function(c)
        count = count + 1
        if M.han(c) and not inventory[c] then allowed = false end
    end)
    return (ordinary and count ~= 1) or allowed
end
function M.apply(context, mode)
    for option, value in pairs(M.options) do
        if value == mode and not context:get_option(option) then context:set_option(option, true) end
    end
    for option, value in pairs(M.options) do
        if value ~= mode and context:get_option(option) then context:set_option(option, false) end
    end
end
function M.activate(context)
    local mode = context and context:get_option("tiger_sentence_characters_full") and "full" or "common"
    if mode ~= M.mode then M.mode = mode; M.revision = M.revision + 1 end
end
return M
