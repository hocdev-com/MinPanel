local phpmyadmin = {}

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

local function web_path(path)
    return tostring(path or ""):gsub("\\", "/")
end

local function ensure_blowfish_secret(seed)
    local source = tostring(seed or "MinPanel phpMyAdmin")
    local alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
    local secret = {}
    for index = 1, 32 do
        local byte = source:byte(((index - 1) % #source) + 1) or index
        local offset = ((byte + index * 17) % #alphabet) + 1
        secret[index] = alphabet:sub(offset, offset)
    end
    return table.concat(secret)
end

local function config_body(ctx)
    local install_dir = ctx.install_dir
    local tmp_dir = web_path(path_join(install_dir, "tmp"))
    local upload_dir = web_path(path_join(install_dir, "upload"))
    local save_dir = web_path(path_join(install_dir, "save"))
    local blowfish_secret = ensure_blowfish_secret(install_dir)

    return string.format([[<?php
/**
 * MinPanel managed phpMyAdmin configuration.
 */
$cfg['blowfish_secret'] = '%s';
$cfg['TempDir'] = '%s';
$cfg['UploadDir'] = '%s';
$cfg['SaveDir'] = '%s';

$i = 0;
$i++;
$cfg['Servers'][$i]['auth_type'] = 'cookie';
$cfg['Servers'][$i]['host'] = '127.0.0.1';
$cfg['Servers'][$i]['port'] = '3306';
$cfg['Servers'][$i]['compress'] = false;
$cfg['Servers'][$i]['AllowNoPassword'] = true;

$cfg['PmaNoRelation_DisableWarning'] = true;
]], blowfish_secret, tmp_dir, upload_dir, save_dir):gsub("\n", "\r\n")
end

function phpmyadmin.on_install(ctx)
    panel.log("Configuring phpMyAdmin...")
    local install_dir = ctx.install_dir

    if not panel.exists(path_join(install_dir, "index.php")) then
        error("phpMyAdmin index.php not found at " .. path_join(install_dir, "index.php"))
    end

    panel.mkdir(path_join(install_dir, "tmp"))
    panel.mkdir(path_join(install_dir, "upload"))
    panel.mkdir(path_join(install_dir, "save"))
    panel.write_file(path_join(install_dir, "config.inc.php"), config_body(ctx))

    return "phpMyAdmin configuration completed"
end

function phpmyadmin.on_start(ctx)
    panel.log("phpMyAdmin is a PHP application and does not run as a standalone service.")
    return "phpMyAdmin is ready"
end

function phpmyadmin.on_stop(ctx)
    panel.log("phpMyAdmin has no standalone service to stop.")
    return "phpMyAdmin stopped"
end

function phpmyadmin.on_uninstall(ctx)
    panel.log("Cleaning up phpMyAdmin generated files...")
    local install_dir = ctx.install_dir
    panel.remove_file(path_join(install_dir, "config.inc.php"))
    return "phpMyAdmin cleanup completed"
end

return phpmyadmin
