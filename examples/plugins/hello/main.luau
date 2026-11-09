--!strict
-- Hello: two local /commands. They only fill the message box; you still
-- press Enter to send.

local SHRUG = "¯\\_(ツ)_/¯"

rustcord.command("shrug", "Appends ¯\\_(ツ)_/¯ to your message", function(args: string)
	if args ~= "" then
		return args .. " " .. SHRUG
	end
	return SHRUG
end)

-- A Discord timestamp shows in each reader's own time zone.
rustcord.command("time", "Your current time (add 'full' for the date too)", function(args: string)
	local style = if args == "full" then "F" else "t"
	return string.format("<t:%d:%s>", os.time(), style)
end)

rustcord.log("hello loaded: /shrug and /time are ready")
