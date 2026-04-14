local php = {}

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
