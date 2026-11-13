-- Wind-up alarm clocks: a new behaviour for an item the game already has.
--
-- Use an alarm clock (it has the `usable` tag in this mod's items.ron) and it is set down at your
-- feet, ticking. DELAY seconds later it rings for RING seconds: a noise every second that zombies
-- within RADIUS metres come to look at (less through walls), loud enough to draw the far horde
-- too. Pick it up and it stops. When it has run down it is an ordinary alarm clock again.
--
-- Everything that must outlast a call is kept with game:set (saved with the world), never in a
-- script variable: see docs/modding.md.

local DELAY = 10 -- seconds from setting it down to the first ring
local RING = 20 -- seconds it rings
local RADIUS = 45 -- metres its ring carries
local TICKS = 20 -- simulation ticks a second

function on_use(game, player, item)
	if item ~= "alarm_clock" and item ~= "alarm_clock_set" then
		return
	end
	local p = game:player(player)
	if not p then
		return
	end
	-- The clock used is gone from the pockets; this is it on the floor.
	local id = game:spawn_item("alarm_clock_set", p.x, p.y)
	if not id then
		return
	end
	local clocks = game:get("clocks") or {}
	table.insert(clocks, { id = id, x = p.x, y = p.y, rings_at = game:tick() + DELAY * TICKS, owner = player })
	game:set("clocks", clocks)
	game:tell(player, "You wind the alarm clock and set it down. It rings in " .. DELAY .. " seconds.")
end

function on_tick(game)
	local clocks = game:get("clocks")
	if not clocks then
		return
	end
	local now = game:tick()
	local keep, changed = {}, false
	for _, c in ipairs(clocks) do
		if not game:ground_item(c.id) then
			-- Somebody picked it up: it stops.
			changed = true
		elseif now >= c.rings_at + RING * TICKS then
			-- Run down: an ordinary alarm clock again.
			game:remove_item(c.id)
			game:spawn_item("alarm_clock", c.x, c.y)
			changed = true
		else
			if now >= c.rings_at and (now - c.rings_at) % TICKS == 0 then
				game:noise(c.x, c.y, RADIUS)
				if now == c.rings_at then
					game:tell(c.owner, "The alarm clock is ringing!")
				end
			end
			table.insert(keep, c)
		end
	end
	if changed then
		game:set("clocks", if #keep > 0 then keep else nil)
	end
end
