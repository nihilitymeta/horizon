-- Asset and settings sample: shows a declared setting that changes behaviour, and a shipped asset.
-- The manifest declares one toggle, "loud"; horizon.setting reads it (the player's value, or the
-- default false). The command `greet` answers in capitals when it is on. The mod also ships
-- lang/en_us.json, which the game publishes under this mod's namespace as an asset.
-- Shows: horizon.setting, horizon.command with run_mod_command, event.feedback, a shipped lang file.

horizon.command({ usage = "greet" })

horizon.on("run_mod_command", function(event)
	local word = "hello"
	if horizon.setting("loud") == true then
		word = "HELLO"
	end
	event.feedback(word .. ", from the asset and settings sample")
	event.ran = true
end)
