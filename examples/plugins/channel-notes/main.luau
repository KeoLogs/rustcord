--!strict
-- Channel Notes: a panel with a notes box for whichever channel is open.

local current: string? = nil
local title = ""

rustcord.on("channel_open", function(ch)
	current = ch.id
	title = if ch.guild_id then "#" .. ch.name else ch.name
end)

rustcord.ui.panel("Channel notes", function(ui)
	local id = current
	if id == nil then
		ui:small("Open a channel to keep notes for it.")
		return
	end
	local key = "note:" .. id
	ui:heading(title)
	ui:textbox(key, "Notes for this channel (only saved on this PC)", true)
	ui:button("Clear", function()
		rustcord.storage.set(key, nil)
	end)
end)
