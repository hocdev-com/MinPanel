local output = [[
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       5356
  TCP    127.0.0.1:80           0.0.0.0:0              LISTENING       5356
  TCP    [::]:80                [::]:0                 LISTENING       5356
]]

function trim(s)
  return s:match("^%s*(.-)%s*$")
end

function contains_ci(s, sub)
  return s:lower():find(sub:lower(), 1, true) ~= nil
end

local listening = {}
for line in output:gmatch("[^\r\n]+") do
    local trimmed = trim(line)
    if contains_ci(trimmed, "LISTENING") then
        local columns = {}
        for col in trimmed:gmatch("%S+") do
            table.insert(columns, col)
        end
        if #columns >= 4 then
            local local_addr = columns[2]
            local pid = columns[#columns]
            local port = local_addr:match(":([%d]+)$")
            if port then
                listening[port] = pid
            end
        end
    end
end

for port, pid in pairs(listening) do
    print("Port: " .. port .. " is used by PID: " .. pid)
end
