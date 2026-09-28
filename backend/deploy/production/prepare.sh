#!/bin/bash
# Initialize new-host configuration. Never replace existing credentials.
set -Eeuo pipefail
umask 077
# Bind mounts retain host permissions, including checkouts made under umask 077.
# These two files contain no credentials and must be readable inside containers.
deployment_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
chmod 0644 "$deployment_dir/Caddyfile"
chmod 0755 "$deployment_dir/init-database.sh"
root=/opt/lycoris
private="$root/private"
reset_password=
while IFS= read -r line; do
    case "$line" in
        ADMIN_DEFAULT_USER_PASSWORD=*) reset_password="${line#*=}" ;;
    esac
done < "${1:?Pass the private admin.env exported from the previous service}"
for name in postgres-password app-db-password app.env; do
    if test -e "$private/$name"; then
        echo 'Production configuration exists; refusing to replace it.' >&2
        exit 1
    fi
done
install -d -m 0700 "$private"
postgres_password="$(openssl rand -hex 32)"
app_password="$(openssl rand -hex 32)"
reset_password="${reset_password:-$(openssl rand -hex 24)}"
printf '%s\n' "$postgres_password" > "$private/postgres-password"
printf '%s\n' "$app_password" > "$private/app-db-password"
# Container secret mounts must be readable by PostgreSQL's UID. The host
# parent directory remains 0700 and the mounts are read-only.
chmod 0444 "$private/postgres-password" "$private/app-db-password"
{
    printf '%s\n' "DATABASE_URL=postgres://lycoris:$app_password@127.0.0.1:15432/lycoris"
    printf '%s\n' 'REDIS_URL=redis://127.0.0.1:16379'
    printf '%s\n' 'WRITE_ALLOWED_ORIGINS=https://lycoris-map.com' 'CORS_ALLOWED_ORIGINS='
    printf '%s\n' 'SESSION_COOKIE_NAME=LYCORIS_SESSION' 'SESSION_COOKIE_SECURE=true'
    printf '%s\n' 'SESSION_COOKIE_DOMAIN=' 'SESSION_COOKIE_SAME_SITE=lax'
    printf '%s\n' 'SESSION_NAMESPACE=lycoris:rust:session:production:v1'
    printf '%s\n' 'RATE_LIMIT_NAMESPACE=lycoris:rust:ratelimit:production:v1'
    printf '%s\n' 'MARKER_CACHE_NAMESPACE=lycoris:rust:marker:production:v1'
    printf '%s\n' 'APP_AVAILABILITY_ZONE=Asia/Shanghai'
    printf '%s\n' "ADMIN_DEFAULT_USER_PASSWORD=$reset_password" 'RUST_LOG=info'
} > "$private/app.env"
chmod 0600 "$private/app.env"
for name in postgres redis uploads caddy caddy-config; do
    install -d -m 0750 "$root/data/$name"
done
chown 10001:10001 "$root/data/uploads"
echo 'Production configuration prepared; no secret values printed.'
