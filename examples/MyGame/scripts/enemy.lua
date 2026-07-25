local self = {}

function self:on_create()
    self:set_color(0.9, 0.2, 0.2, 1.0)
    engine:log("enemy spawned")
end

function self:on_update(dt)
    local x, y, z = self:get_position()
    self:set_position(x, y + math.sin(engine:get_dt()) * 0.5, z)
end

return self
