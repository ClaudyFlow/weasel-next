-- Three deletion units shared by the processor and persistent Rime radio group.
local M = {}
M.options = {
    tiger_sentence_backspace_code = "code",
    tiger_sentence_backspace_character = "character",
    tiger_sentence_backspace_code_then_character = "code_then_character"
}
M.path = "options/tiger_sentence_backspace_unit"
local active = "tiger_sentence_backspace_run"

function M.valid(unit)
    return unit == "code" or unit == "character" or unit == "code_then_character"
end

function M.unit(context)
    local selected, count = "character", 0
    for option, unit in pairs(M.options) do
        if context:get_option(option) then selected, count = unit, count + 1 end
    end
    return count == 1 and selected or "character"
end

function M.interrupt(context)
    if (context:get_property(active) or "") ~= "" then context:set_property(active, "") end
end

function M.reset(context, env)
    M.interrupt(context)
    if env then env._tiger_backspace = nil end
end

function M.observe(key, context)
    -- The ASCII composer runs first and may consume a modifier/ASCII key.
    -- Only an interruption writes the shared token, never each deleted key.
    if not key:release() and key:repr() ~= "BackSpace" then M.interrupt(context) end
end

function M.apply(context, unit)
    if context:get_property("tiger_sentence_backspace_unit") ~= unit then M.interrupt(context) end
    for option, value in pairs(M.options) do
        if value == unit and not context:get_option(option) then context:set_option(option, true) end
    end
    for option, value in pairs(M.options) do
        if value ~= unit and context:get_option(option) then context:set_option(option, false) end
    end
    if context:get_property("tiger_sentence_backspace_unit") ~= unit then
        context:set_property("tiger_sentence_backspace_unit", unit)
    end
end

function M.last_span(candidate, length, floor)
    local node = candidate and candidate.path
    if not node or node.raw_length ~= length then return 1 end
    local previous = node.previous and node.previous.raw_length or 0
    if type(previous) ~= "number" or previous ~= math.floor(previous) then return 1 end
    local start = math.max(floor, previous)
    return start >= 0 and start < length and length - start or 1
end

function M.count(context, env, raw, caret, identity, span)
    local unit = M.unit(context)
    -- Windows edits the tail. Preserve Rime's existing caret-relative code
    -- editing in the middle, including the private buffered-text marker.
    if raw == "" or caret ~= #raw or unit ~= "code_then_character" then
        M.reset(context, env)
        return unit == "character" and caret == #raw and span or 1
    end
    local run = env._tiger_backspace
    if not run or context:get_property(active) ~= "1" or
        run.raw ~= raw or run.identity ~= identity then
        run = {remaining=span, identity=identity}
        env._tiger_backspace = run
        if context:get_property(active) ~= "1" then context:set_property(active, "1") end
    end
    local count = span
    if run.remaining > 0 then
        run.remaining = run.remaining - 1
        count = 1
    end
    -- Capture once: subsequent resegmentation cannot extend the initial span.
    run.raw = raw:sub(1, #raw - count)
    return count
end

return M
