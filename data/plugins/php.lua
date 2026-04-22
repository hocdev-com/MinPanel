local php = {}

local function path_join(base, ...)
    local result = tostring(base or "")
    for index = 1, select("#", ...) do
        local part = tostring(select(index, ...) or "")
        if part ~= "" then
            if result == "" then
                result = part
            else
                result = result:gsub("[\\/]+$", "") .. "\\" .. part:gsub("^[\\/]+", "")
            end
        end
    end
    return result
end

local function normalize_php_ini(contents, install_dir)
    contents = tostring(contents or ""):gsub("\r\n", "\n"):gsub("\r", "\n")
    local ext_dir = path_join(install_dir, "ext"):gsub("\\", "/")
    local wanted = {
        extension_dir = 'extension_dir = "' .. ext_dir .. '"',
        mysqli = "extension=mysqli",
        mbstring = "extension=mbstring",
        openssl = "extension=openssl",
    }
    local seen = {}
    local lines = {}

    for line in (contents .. "\n"):gmatch("(.-)\n") do
        local trimmed = line:gsub("^%s+", ""):gsub("%s+$", "")
        local uncommented = trimmed:gsub("^%s*;+%s*", "")
        local lower = uncommented:lower()

        if lower:match("^extension_dir%s*=") then
            if not seen.extension_dir then
                table.insert(lines, wanted.extension_dir)
                seen.extension_dir = true
            end
        elseif lower == "extension=mysqli" or lower == "extension=php_mysqli.dll" then
            if not seen.mysqli then
                table.insert(lines, wanted.mysqli)
                seen.mysqli = true
            end
        elseif lower == "extension=mbstring" or lower == "extension=php_mbstring.dll" then
            if not seen.mbstring then
                table.insert(lines, wanted.mbstring)
                seen.mbstring = true
            end
        elseif lower == "extension=openssl" or lower == "extension=php_openssl.dll" then
            if not seen.openssl then
                table.insert(lines, wanted.openssl)
                seen.openssl = true
            end
        else
            table.insert(lines, line)
        end
    end

    if not seen.extension_dir then table.insert(lines, wanted.extension_dir) end
    if not seen.mysqli then table.insert(lines, wanted.mysqli) end
    if not seen.mbstring then table.insert(lines, wanted.mbstring) end
    if not seen.openssl then table.insert(lines, wanted.openssl) end

    return table.concat(lines, "\r\n") .. "\r\n"
end

function php.on_install(ctx)
    panel.log("Configuring PHP environment...")
    local install_dir = ctx.install_dir

    panel.mkdir(install_dir .. "\\logs")
    panel.mkdir(install_dir .. "\\tmp")
    panel.mkdir(install_dir .. "\\tmp\\session")
    panel.mkdir(install_dir .. "\\tmp\\upload")

    local php_ini = install_dir .. "\\php.ini"
    if not panel.exists(php_ini) then
        panel.log("Initializing php.ini from template...")
        local template = install_dir .. "\\php.ini-production"
        if panel.exists(template) then
            panel.copy_file(template, php_ini)
        end
    end
    if panel.exists(php_ini) then
        panel.log("Enabling PHP extensions required by phpMyAdmin...")
        panel.write_file(php_ini, normalize_php_ini(panel.read_file(php_ini), install_dir))
    end

    return "PHP configuration completed"
end

function php.on_start(ctx)
    panel.log("Starting PHP FastCGI...")
    local port = ctx.port or "9000"
    local install_dir = ctx.install_dir
    local bin = install_dir .. "\\php-cgi.exe"
    local ini = install_dir .. "\\php.ini"

    if not panel.exists(bin) then
        return "Error: php-cgi.exe not found at " .. bin
    end

    local pid = panel.spawn(bin, {"-b", "127.0.0.1:" .. port, "-c", ini})
    panel.log("PHP spawned with PID " .. tostring(pid))

    local pid_file = install_dir .. "\\logs\\php-cgi.pid"
    panel.write_file(pid_file, tostring(pid))

    return "PHP started (PID " .. tostring(pid) .. ", port " .. port .. ")"
end

function php.on_stop(ctx)
    panel.log("Stopping PHP...")
    local install_dir = ctx.install_dir
    panel.execute("taskkill", {"/F", "/IM", "php-cgi.exe", "/T"})

    local pid_file = install_dir .. "\\logs\\php-cgi.pid"
    if panel.exists(pid_file) then
        panel.write_file(pid_file, "")
    end
    return "PHP stopped"
end

function php.on_uninstall(ctx)
    panel.log("Cleaning up PHP...")
    return "PHP cleanup completed"
end

return php
