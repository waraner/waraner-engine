local self = {}

function self:on_create()
    self:set_position(0, 1.5, 0)
    self:set_color(0.2, 0.6, 1.0, 1.0)
    engine:log("player spawned")
end

function self:on_update(dt)
    local speed = 5.0 * dt
    local dx, dy, dz = 0, 0, 0

    if engine.input:is_key_down("W") then dz = dz - speed end
    if engine.input:is_key_down("S") then dz = dz + speed end
    if engine.input:is_key_down("A") then dx = dx - speed end
    if engine.input:is_key_down("D") then dx = dx + speed end

    local x, y, z = self:get_position()
    self:set_position(x + dx, y, z + dz)
end

return self
