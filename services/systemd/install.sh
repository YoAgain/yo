#!/bin/sh
# Copies the units for the bots named: install.sh DIR ping yogpt … (DIR: /etc/systemd/system). Starts nothing.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
dir=$1
shift
install -m 644 "$here"/*.slice "$here"/yo-service@.service "$here"/yo-entrance.service "$dir"
install -d "$dir/yo-scripts-.slice.d"
install -m 644 "$here/one-script.conf" "$dir/yo-scripts-.slice.d"
for bot; do
    case $bot in *[!a-z0-9]* | "") echo "not a bot: $bot" >&2; exit 1 ;; esac
    install -m 644 "$here/script.socket" "$dir/yo-script-$bot.socket"
    service=$bot
    [ "$service" = pingtest ] && service=ping           # another name of PING
    script=$(ls -d /opt/yo-services/*/"$service"/"$service.py")
    folder=${script%/*}
    install -d "$dir/yo-service@$bot.service.d"
    printf '[Service]\nEnvironment=SERVICE=%s\n' "${folder#/opt/yo-services/}" > "$dir/yo-service@$bot.service.d/folder.conf"
    if grep -q '^\[bridge\]' "${script%.py}.toml"; then
        install -m 644 "$here/online.conf" "$dir/yo-service@$bot.service.d"
    fi
    sed "s#@SCRIPT@#$script#" "$here/script.service" > "$dir/yo-script-$bot@.service"
done
# /var/lib/yo-services/accounts/BOT: its keys, its own user's alone. db: one per hub, written by all executors.
# /etc/yo-services/hub.env: the hub; BOT.env: of a bridge, its SERVICES_KEY and SERVICES_CLIENT_SECRET. Both root's.
getent group yo-services > /dev/null || groupadd -r yo-services
install -m 644 "$here/yo-services.conf" /etc/tmpfiles.d/
systemd-tmpfiles --create yo-services.conf
install -d -m 755 /var/lib/yo-services /var/lib/yo-services/accounts
install -d -m 2770 -g yo-services /var/lib/yo-services/db
install -d -m 700 /etc/yo-services
for book in live; do
    book=/var/lib/yo-services/db/$book.sqlite
    [ -f "$book" ] || install -m 660 -g yo-services /dev/null "$book"
done
for bot; do
    id -u "yo-service-$bot" > /dev/null 2>&1 ||
        useradd -r -M -d /nonexistent -s /usr/sbin/nologin -G yo-services "yo-service-$bot"
done
