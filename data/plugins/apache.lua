local apache = {}

local function trim(value)
    value = tostring(value or "")
    value = value:gsub("^%s+", "")
    value = value:gsub("%s+$", "")
    return value
end

local function starts_with_ci(value, prefix)
    value = tostring(value or "")
    prefix = tostring(prefix or "")
    return value:sub(1, #prefix):lower() == prefix:lower()
end

local function contains_ci(value, needle)
    value = tostring(value or "")
    needle = tostring(needle or "")
    return needle ~= "" and value:lower():find(needle:lower(), 1, true) ~= nil
end

local function uncommented_value(value)
    return trim(tostring(value or ""):gsub("^#+", ""))
end

local function split_lines(contents)
    local normalized = tostring(contents or ""):gsub("\r\n", "\n"):gsub("\r", "\n")
    local lines = {}
    if normalized == "" then
        return lines
    end
    for line in (normalized .. "\n"):gmatch("(.-)\n") do
        table.insert(lines, line)
    end
    return lines
end

local function join_lines(lines)
    local body = table.concat(lines, "\r\n")
    if not body:match("\r\n$") then
        body = body .. "\r\n"
    end
    return body
end

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

local function path_basename(path)
    return tostring(path or ""):match("([^\\/]+)$") or tostring(path or "")
end

local function sanitize_segment(value)
    local sanitized = tostring(value or ""):gsub("[^%w%._%-]+", "-")
    sanitized = sanitized:gsub("^%-+", ""):gsub("%-+$", "")
    if sanitized == "" then
        sanitized = "site"
    end
    return sanitized
end

local function apache_path(path)
    return tostring(path or ""):gsub("\\", "/")
end

local function normalize_module_line(trimmed, module_name, module_path)
    local uncommented = uncommented_value(trimmed)
    local directive, name = uncommented:match("^(%S+)%s+(%S+)")
    if directive and directive:lower() == "loadmodule" and name:lower() == module_name:lower() then
        return "LoadModule " .. module_name .. " " .. module_path
    end
    return nil
end

local function push_managed_include_lines(lines, state)
    if state.inserted_managed_includes then
        return
    end
    table.insert(lines, "IncludeOptional conf/extra/httpd-MinPanel.conf")
    table.insert(lines, "IncludeOptional conf/vhost/*.conf")
    state.inserted_managed_includes = true
end

local function normalize_httpd_conf(contents, install_dir, enable_ssl)
    local server_root = apache_path(install_dir)
    local state = {
        saw_define_srvroot = false,
        saw_server_root = false,
        saw_listen_80 = false,
        saw_listen_443 = false,
        saw_server_name = false,
        saw_directory_index = false,
        saw_proxy_module = false,
        saw_proxy_fcgi_module = false,
        saw_rewrite_module = false,
        saw_ssl_module = false,
        saw_socache_shmcb_module = false,
        saw_headers_module = false,
        inserted_managed_includes = false,
    }
    local lines = {}

    for _, line in ipairs(split_lines(contents)) do
        local trimmed = trim(line)
        local uncommented = uncommented_value(trimmed)

        local module_line = normalize_module_line(trimmed, "proxy_module", "modules/mod_proxy.so")
        if module_line then
            if not state.saw_proxy_module then
                state.saw_proxy_module = true
                table.insert(lines, module_line)
            end
            goto continue
        end

        module_line = normalize_module_line(trimmed, "proxy_fcgi_module", "modules/mod_proxy_fcgi.so")
        if module_line then
            if not state.saw_proxy_fcgi_module then
                state.saw_proxy_fcgi_module = true
                table.insert(lines, module_line)
            end
            goto continue
        end

        module_line = normalize_module_line(trimmed, "rewrite_module", "modules/mod_rewrite.so")
        if module_line then
            if not state.saw_rewrite_module then
                state.saw_rewrite_module = true
                table.insert(lines, module_line)
            end
            goto continue
        end

        module_line = normalize_module_line(trimmed, "ssl_module", "modules/mod_ssl.so")
        if module_line then
            if enable_ssl and not state.saw_ssl_module then
                state.saw_ssl_module = true
                table.insert(lines, module_line)
            end
            goto continue
        end

        module_line = normalize_module_line(trimmed, "headers_module", "modules/mod_headers.so")
        if module_line then
            if not state.saw_headers_module then
                state.saw_headers_module = true
                table.insert(lines, module_line)
            end
            goto continue
        end

        module_line = normalize_module_line(trimmed, "socache_shmcb_module", "modules/mod_socache_shmcb.so")
        if module_line then
            if enable_ssl and not state.saw_socache_shmcb_module then
                state.saw_socache_shmcb_module = true
                table.insert(lines, module_line)
            end
            goto continue
        end

        if starts_with_ci(uncommented, "LoadModule php_module ")
            or starts_with_ci(uncommented, "PHPIniDir ")
            or starts_with_ci(uncommented, "AddHandler application/x-httpd-php ")
            or starts_with_ci(uncommented, "AddType application/x-httpd-php ")
            or starts_with_ci(uncommented, "SSLRandomSeed ")
        then
            goto continue
        end

        if uncommented:lower() == "virtual hosts" then
            table.insert(lines, line)
            push_managed_include_lines(lines, state)
            goto continue
        end

        if starts_with_ci(uncommented, "Define SRVROOT ") then
            if not state.saw_define_srvroot then
                state.saw_define_srvroot = true
                if not state.saw_server_root then
                    table.insert(lines, string.format('Define SRVROOT "%s"', server_root))
                end
            end
            goto continue
        end

        if starts_with_ci(uncommented, "ServerRoot ") then
            if not state.saw_define_srvroot then
                table.insert(lines, string.format('Define SRVROOT "%s"', server_root))
                state.saw_define_srvroot = true
            end
            if not state.saw_server_root then
                state.saw_server_root = true
                table.insert(lines, 'ServerRoot "${SRVROOT}"')
            end
            goto continue
        end

        if starts_with_ci(uncommented, "Listen ") then
            local port_part = trim(uncommented:sub(#"Listen " + 1))
            local last = port_part:match("([^:]+)$") or port_part
            local port = trim((last:match("^(%S+)") or last))
            if port == "80" then
                if not state.saw_listen_80 then
                    table.insert(lines, "Listen 127.0.0.1:80")
                    state.saw_listen_80 = true
                end
                goto continue
            elseif port == "443" then
                if enable_ssl and not state.saw_listen_443 then
                    table.insert(lines, "Listen 127.0.0.1:443")
                    state.saw_listen_443 = true
                end
                goto continue
            end
        end

        if starts_with_ci(uncommented, "ServerName ") then
            if not state.saw_server_name then
                table.insert(lines, "ServerName 127.0.0.1:80")
                state.saw_server_name = true
            end
            goto continue
        end

        if starts_with_ci(uncommented, "DirectoryIndex ") then
            if not state.saw_directory_index then
                table.insert(lines, "DirectoryIndex index.php index.html index.htm")
                state.saw_directory_index = true
            end
            goto continue
        end

        local lower = uncommented:lower()
        if lower == "include conf/extra/httpd-vhosts.conf"
            or lower == "include conf/extra/httpd-ssl.conf"
            or lower == "includeoptional conf/vhost/*.conf"
            or lower == "includeoptional conf/extra/httpd-minpanel.conf"
        then
            goto continue
        end

        table.insert(lines, line)

        ::continue::
    end

    if not state.saw_define_srvroot then
        table.insert(lines, string.format('Define SRVROOT "%s"', server_root))
    end
    if not state.saw_server_root then
        table.insert(lines, 'ServerRoot "${SRVROOT}"')
    end
    if not state.saw_listen_80 then
        table.insert(lines, "Listen 127.0.0.1:80")
    end
    if enable_ssl and not state.saw_listen_443 then
        table.insert(lines, "Listen 127.0.0.1:443")
    end
    if not state.saw_server_name then
        table.insert(lines, "ServerName 127.0.0.1:80")
    end
    if not state.saw_directory_index then
        table.insert(lines, "DirectoryIndex index.php index.html index.htm")
    end
    if not state.saw_proxy_module then
        table.insert(lines, "LoadModule proxy_module modules/mod_proxy.so")
    end
    if not state.saw_proxy_fcgi_module then
        table.insert(lines, "LoadModule proxy_fcgi_module modules/mod_proxy_fcgi.so")
    end
    if not state.saw_rewrite_module then
        table.insert(lines, "LoadModule rewrite_module modules/mod_rewrite.so")
    end
    if enable_ssl and not state.saw_ssl_module then
        table.insert(lines, "LoadModule ssl_module modules/mod_ssl.so")
        table.insert(lines, "SSLRandomSeed startup builtin")
        table.insert(lines, "SSLRandomSeed connect builtin")
    end
    if enable_ssl and not state.saw_socache_shmcb_module then
        table.insert(lines, "LoadModule socache_shmcb_module modules/mod_socache_shmcb.so")
    end
    if not state.saw_headers_module then
        table.insert(lines, "LoadModule headers_module modules/mod_headers.so")
    end
    push_managed_include_lines(lines, state)

    return join_lines(lines)
end

local function render_extra_conf(website_root, apache_logs_root)
    local document_root = apache_path(website_root)
    local error_log = apache_path(path_join(apache_logs_root, "error.log"))
    local access_log = apache_path(path_join(apache_logs_root, "access.log"))

    return string.format([[
# MinPanel managed Apache extras
ServerName 127.0.0.1:80
DocumentRoot "%s"
DirectoryIndex index.php index.html index.htm
ErrorLog "%s"
CustomLog "%s" common

<Directory "%s">
    AllowOverride All
    Options Indexes FollowSymLinks
    Require all granted
</Directory>

<IfModule proxy_module>
    ProxyTimeout 60
</IfModule>
]], document_root, error_log, access_log, document_root):gsub("\n", "\r\n")
end

local function detect_site_document_root(site_root)
    for _, child in ipairs({"public", "public_html", "htdocs", "www", "web"}) do
        local candidate = path_join(site_root, child)
        if panel.is_dir(candidate) then
            return candidate
        end
    end
    return site_root
end

local function render_php_block(site)
    if not site.php_port then
        return ""
    end

    return string.format([[
    <Proxy "fcgi://127.0.0.1:%s/" enablereuse=on max=10>
    </Proxy>
    ProxyFCGIBackendType GENERIC
    ProxyFCGISetEnvIf "true" SCRIPT_FILENAME "%%{reqenv:DOCUMENT_ROOT}%%{reqenv:SCRIPT_NAME}"
    ProxyFCGISetEnvIf "true" PATH_TRANSLATED "%%{reqenv:DOCUMENT_ROOT}%%{reqenv:SCRIPT_NAME}"
    <FilesMatch "\.php$">
        SetHandler "proxy:fcgi://127.0.0.1:%s/"
    </FilesMatch>]], tostring(site.php_port), tostring(site.php_port))
end

local function render_vhost(site, logs_root)
    local document_root = detect_site_document_root(site.path)
    local document_root_value = apache_path(document_root)
    local site_log_base = sanitize_segment(site.id)
    local error_log = apache_path(path_join(logs_root, site_log_base .. "-error.log"))
    local access_log = apache_path(path_join(logs_root, site_log_base .. "-access.log"))
    local php_block = render_php_block(site)

    local http_vhost = string.format([[
# MinPanel managed site: %s
<VirtualHost 127.0.0.1:80>
    ServerName %s
    DocumentRoot "%s"
    DirectoryIndex index.php index.html index.htm
    <Directory "%s">
        Options FollowSymLinks ExecCGI
        AllowOverride All
        Require all granted
    </Directory>
%s

    ErrorLog "%s"
    CustomLog "%s" common
</VirtualHost>
]], site.id, site.domain, document_root_value, document_root_value, php_block, error_log, access_log)

    local https_vhost = ""
    if site.ssl and site.ssl.cert and site.ssl.key then
        local cert = apache_path(site.ssl.cert)
        local key = apache_path(site.ssl.key)
        local ssl_error_log = apache_path(path_join(logs_root, site_log_base .. "-ssl-error.log"))
        local ssl_access_log = apache_path(path_join(logs_root, site_log_base .. "-ssl-access.log"))
        https_vhost = string.format([[

# MinPanel SSL site: %s
<VirtualHost 127.0.0.1:443>
    ServerName %s
    DocumentRoot "%s"
    DirectoryIndex index.php index.html index.htm
    <Directory "%s">
        Options FollowSymLinks ExecCGI
        AllowOverride All
        Require all granted
    </Directory>

    SSLEngine on
    SSLCertificateFile "%s"
    SSLCertificateKeyFile "%s"
    SSLProtocol all -SSLv3 -TLSv1 -TLSv1.1
    SSLCipherSuite HIGH:!aNULL:!MD5
    SSLSessionCacheTimeout 300
    Header always set Strict-Transport-Security "max-age=31536000"
%s

    ErrorLog "%s"
    CustomLog "%s" common
</VirtualHost>
]], site.id, site.domain, document_root_value, document_root_value, cert, key, php_block, ssl_error_log, ssl_access_log)
    end

    return (http_vhost .. https_vhost):gsub("\n", "\r\n")
end

local function write_if_changed(path, content)
    local current = panel.exists(path) and panel.read_file(path) or nil
    if current ~= content then
        panel.write_file(path, content)
    end
end

local function cleanup_managed_vhosts(vhost_root)
    if not panel.is_dir(vhost_root) then
        return
    end
    for _, entry_path in ipairs(panel.read_dir(vhost_root)) do
        local name = path_basename(entry_path)
        if name:match("^MinPanel%-.*%.conf$") then
            panel.remove_file(entry_path)
        end
    end
end

local function managed_ssl_vhost_exists(vhost_root)
    if not panel.is_dir(vhost_root) then
        return false
    end

    for _, entry_path in ipairs(panel.read_dir(vhost_root)) do
        local name = path_basename(entry_path)
        if name:match("^MinPanel%-.*%.conf$") and panel.exists(entry_path) then
            local ok, body = pcall(panel.read_file, entry_path)
            if ok and (contains_ci(body, "<VirtualHost 127.0.0.1:443>") or contains_ci(body, "SSLEngine on")) then
                return true
            end
        end
    end

    return false
end

local function runtime_requires_ssl(ctx, vhost_root)
    for _, site in ipairs(ctx.sites or {}) do
        if site.enabled ~= false and site.ssl and site.ssl.cert and site.ssl.key then
            return true
        end
    end
    return managed_ssl_vhost_exists(vhost_root)
end

local function last_non_empty_line(contents)
    local lines = split_lines(contents)
    for index = #lines, 1, -1 do
        local line = trim(lines[index])
        if line ~= "" then
            return line
        end
    end
    return ""
end

local function read_apache_error_hint(install_dir, runtime)
    local candidates = {
        path_join(install_dir, "logs", "error.log"),
        path_join(runtime.apache_logs_root or "", "error.log"),
    }

    for _, path in ipairs(candidates) do
        if path ~= "" and panel.exists(path) then
            local ok, contents = pcall(panel.read_file, path)
            if ok then
                local line = last_non_empty_line(contents)
                if line ~= "" then
                    return line
                end
            end
        end
    end

    return ""
end

local function ensure_runtime_config(ctx)
    local install_dir = ctx.install_dir
    local website_root = ctx.website_root or path_join(ctx.data_root or "", "www")
    local apache_logs_root = path_join(ctx.data_root or "", "logs", "apache")
    local site_logs_root = path_join(apache_logs_root, "sites")
    local httpd_conf = path_join(install_dir, "conf", "httpd.conf")
    local extra_conf_path = path_join(install_dir, "conf", "extra", "httpd-MinPanel.conf")
    local vhost_root = path_join(install_dir, "conf", "vhost")

    if not panel.exists(httpd_conf) then
        return nil, "Apache config not found at " .. httpd_conf
    end

    panel.mkdir(vhost_root)
    panel.mkdir(path_join(install_dir, "conf", "extra"))
    panel.mkdir(website_root)
    panel.mkdir(apache_logs_root)
    panel.mkdir(site_logs_root)

    local enable_ssl = runtime_requires_ssl(ctx, vhost_root)
    local normalized = normalize_httpd_conf(panel.read_file(httpd_conf), install_dir, enable_ssl)
    write_if_changed(httpd_conf, normalized)
    write_if_changed(extra_conf_path, render_extra_conf(website_root, apache_logs_root))

    return {
        httpd_conf = httpd_conf,
        vhost_root = vhost_root,
        apache_logs_root = apache_logs_root,
        site_logs_root = site_logs_root,
        enable_ssl = enable_ssl,
    }
end

function apache.sync_sites(ctx)
    panel.log("Synchronizing Apache site routing...")
    local runtime, runtime_error = ensure_runtime_config(ctx)
    if not runtime then
        return runtime_error or "Apache runtime configuration could not be prepared"
    end

    cleanup_managed_vhosts(runtime.vhost_root)

    for _, site in ipairs(ctx.sites or {}) do
        if site.enabled ~= false then
            local body = render_vhost(site, runtime.site_logs_root)
            local target = path_join(runtime.vhost_root, "MinPanel-" .. sanitize_segment(site.id) .. ".conf")
            panel.write_file(target, body)
        end
    end

    return "Apache site routing synchronized"
end

function apache.on_install(ctx)
    panel.log("Configuring Apache directories...")
    panel.mkdir(path_join(ctx.install_dir, "logs"))
    panel.mkdir(path_join(ctx.install_dir, "conf", "vhost"))
    if ctx.data_root then
        panel.mkdir(path_join(ctx.data_root, "logs", "apache"))
        panel.mkdir(path_join(ctx.data_root, "logs", "apache", "sites"))
    end
    return "Apache configuration completed"
end

function apache.on_start(ctx)
    panel.log("Starting Apache...")
    local runtime, runtime_error = ensure_runtime_config(ctx)
    if not runtime then
        error(runtime_error or "Apache configuration failed")
    end

    local install_dir = ctx.install_dir
    local bin = path_join(install_dir, "bin", "httpd.exe")
    local conf = runtime.httpd_conf

    if not panel.exists(bin) then
        error("httpd.exe not found at " .. bin)
    end
    if not panel.exists(conf) then
        error("httpd.conf not found at " .. conf)
    end

    panel.log("Validating Apache configuration...")
    local check = panel.execute(bin, {"-t", "-d", install_dir, "-f", conf})
    local combined = (check.stdout or "") .. "\n" .. (check.stderr or "")
    
    -- On Windows, httpd -t might return non-zero if port 80 is busy, 
    -- but still print 'Syntax OK' or just some notices.
    local is_ok = (check.code == 0) or contains_ci(combined, "Syntax OK")

    if not is_ok then
        local detail = trim(check.stderr ~= "" and check.stderr or check.stdout)
        local hint = read_apache_error_hint(install_dir, runtime)
        
        -- Differentiate between a real syntax error and a port conflict
        if contains_ci(detail, "AH00072") or contains_ci(hint, "AH00072") or contains_ci(detail, "make_sock: could not bind") then
            error("Apache failed to start: Port 80 (or another configured port) is already in use by another process. Please stop any existing web servers (like IIS or another Apache) and try again.")
        end

        if hint ~= "" and not contains_ci(detail, hint) then
            -- AH00354 is a normal notice on Windows and shouldn't be treated as a fatal error 
            -- if we think the config test failed for other reasons.
            if not contains_ci(hint, "AH00354") or detail == "" then
                detail = detail ~= "" and (detail .. " | " .. hint) or hint
            end
        end
        
        if detail ~= "" then
            panel.log("Config validation failed: " .. detail)
            error("Apache config test failed: " .. detail)
        else
            -- If we have no output but a non-zero code, it's likely a missing dependency or port conflict
            error("Apache config test failed with exit code " .. tostring(check.code) .. ". This often happens if Port 80 is occupied or Visual C++ Redistributable is missing.")
        end
    elseif check.code ~= 0 then
        panel.log("Apache config test returned non-zero code but reported Syntax OK. Proceeding...")
    end

    local ok, pid_or_error = pcall(panel.spawn_detached, bin, {"-d", install_dir, "-f", conf})
    if not ok then
        local detail = trim(tostring(pid_or_error or "Apache process failed to launch"))
        local hint = read_apache_error_hint(install_dir, runtime)
        if hint ~= "" and not contains_ci(detail, hint) then
            detail = detail ~= "" and (detail .. " | " .. hint) or hint
        end
        error("Apache failed to launch: " .. detail)
    end
    panel.log("Apache launch request completed (PID " .. tostring(pid_or_error) .. ")")
    return "Apache launch request completed"
end

function apache.on_stop(ctx)
    panel.log("Stopping Apache...")
    local install_dir = ctx.install_dir
    panel.execute("taskkill", {"/F", "/IM", "httpd.exe", "/T"})

    local pid_file = path_join(install_dir, "logs", "httpd.pid")
    if panel.exists(pid_file) then
        panel.write_file(pid_file, "")
    end
    return "Apache stopped"
end

function apache.on_uninstall(ctx)
    panel.log("Cleaning up Apache...")
    local install_dir = ctx.install_dir
    local vhost_root = path_join(install_dir, "conf", "vhost")
    cleanup_managed_vhosts(vhost_root)
    panel.remove_file(path_join(install_dir, "conf", "extra", "httpd-MinPanel.conf"))
    panel.remove_file(path_join(install_dir, "logs", "httpd.pid"))
    return "Apache cleanup completed"
end

return apache
