local mysql = {}

function mysql.on_install(ctx)
    panel.log("Configuring MySQL environment...")
    local install_dir = ctx.install_dir
    local data_dir = install_dir .. "\\data"
    local bin = install_dir .. "\\bin\\mysqld.exe"

    local my_ini = install_dir .. "\\my.ini"
    if not panel.exists(my_ini) then
        panel.log("Generating my.ini...")
        local unix_base = panel.to_unix_path(install_dir)
        local unix_data = panel.to_unix_path(data_dir)
        local content = "[mysqld]\n" ..
                        "basedir=\"" .. unix_base .. "\"\n" ..
                        "datadir=\"" .. unix_data .. "\"\n" ..
                        "port=3306\n" ..
                        "max_connections=200\n" ..
                        "character-set-server=utf8mb4\n" ..
                        "default-storage-engine=INNODB\n"
        panel.write_file(my_ini, content)
    end

    if not panel.exists(data_dir) then
        panel.log("Initializing MySQL data directory (this may take a moment)...")
        local res = panel.execute(bin, {
            "--initialize-insecure",
            "--basedir=" .. install_dir,
            "--datadir=" .. data_dir
        })
        if res.code ~= 0 then
            return "MySQL initialization failed: " .. res.stderr
        end
    end

    return "MySQL configuration completed"
end

function mysql.on_start(ctx)
    panel.log("Starting MySQL...")
    local install_dir = ctx.install_dir
    local bin = install_dir .. "\\bin\\mysqld.exe"

    panel.spawn(bin, {
        "--standalone",
        "--console",
        "--basedir=" .. install_dir,
        "--datadir=" .. install_dir .. "\\data"
    })
    return "MySQL start command issued"
end

function mysql.on_stop(ctx)
    panel.log("Stopping MySQL...")
    panel.execute("taskkill", {"/F", "/IM", "mysqld.exe", "/T"})
    return "MySQL stopped"
end

function mysql.on_uninstall(ctx)
    panel.log("Cleaning up MySQL...")
    return "MySQL cleanup completed"
end

return mysql
