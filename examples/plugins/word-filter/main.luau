--!strict
-- Word Filter: changes how messages are *shown* on this PC. Nothing is
-- changed on Discord. Words are set in Settings -> Plugins -> Word Filter.

local MAGIC = "^$()%.[]*+-?"

-- Case-insensitive, literal Lua pattern for `word`.
local function pattern(word: string): string
	local out = {}
	for _, code in utf8.codes(word) do
		local c = utf8.char(code)
		if c:match("%a") then
			table.insert(out, "[" .. c:lower() .. c:upper() .. "]")
		elseif #c == 1 and MAGIC:find(c, 1, true) then
			table.insert(out, "%" .. c)
		else
			table.insert(out, c)
		end
	end
	return table.concat(out)
end

local function words(): { string }
	local list = {}
	local raw = rustcord.storage.get("words")
	if type(raw) ~= "string" then
		return list
	end
	for w in raw:gmatch("[^,\n]+") do
		w = w:match("^%s*(.-)%s*$") or ""
		if w ~= "" then
			table.insert(list, w)
		end
	end
	return list
end

rustcord.display(function(msg)
	local list = words()
	if #list == 0 then
		return nil
	end
	local replace = rustcord.storage.get("replace") == true
	local out, hits = msg.content, 0
	for _, w in list do
		local n
		out, n = out:gsub("%f[%w]" .. pattern(w) .. "%f[%W]", function(m: string)
			if replace then
				return string.rep("•", utf8.len(m) or #m)
			end
			return "||" .. m .. "||"
		end)
		hits += n
	end
	if hits == 0 then
		return nil
	end
	return out
end)

rustcord.ui.settings(function(ui)
	ui:label("Words to hide (separate with commas or new lines)")
	ui:textbox("words", "e.g. spoilers, darn", true)
	ui:checkbox("replace", "Replace with dots instead of blurring them as spoilers")
	ui:small("Only changes what you see on this PC. Messages on Discord stay as they are.")
end)
