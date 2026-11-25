#!/bin/sh
# Copies the units for the bots named: install.sh DIR ping yogpt … (DIR: /etc/systemd/system). Starts nothing.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
dir=$1
shift
install -m 644 "$here"/*.slice "$here"/yo-service@.service "$dir"
install -d "$dir/yo-scripts-.slice.d"
install -m 644 "$here/one-script.conf" "$dir/yo-scripts-.slice.d"
for bot; do
    case $bot in *[!a-z0-9]* | "") echo "not a bot: $bot" >&2; exit 1 ;; esac
    install -m 644 "$here/script.socket" "$dir/yo-script-$bot.socket"
    service=${bot%dev}                                  # pingdev: ping on the dev hub
    [ "$service" = pingtest ] && service=ping           # another name of PING
    script=/opt/yo-services/yo/$service/$service.py
    sed "s#@SCRIPT@#$script#" "$here/script.service" > "$dir/yo-script-$bot@.service"
done
# /var/lib/yo-services/accounts/BOT: its keys, its own user's alone. db: one per hub, written by all executors.
# /etc/yo-services/BOT.env: its settings, root's.
getent group yo-services > /dev/null || groupadd -r yo-services
install -d -m 755 /var/lib/yo-services /var/lib/yo-services/accounts
install -d -m 2770 -g yo-services /var/lib/yo-services/db
install -d -m 700 /etc/yo-services
for book in live dev; do
    book=/var/lib/yo-services/db/$book.sqlite
    [ -f "$book" ] || install -m 660 -g yo-services /dev/null "$book"
done
for bot; do
    id -u "yo-service-$bot" > /dev/null 2>&1 ||
        useradd -r -M -d /nonexistent -s /usr/sbin/nologin -G yo-services "yo-service-$bot"
done
# Only STATUS sees the numbers it draws.
case " $* " in *" status "*)
    install -d "$dir/yo-script-status@.service.d"
    install -m 644 "$here/status-numbers.conf" "$dir/yo-script-status@.service.d"
    install -m 644 "$here"/yo-status-collect.* "$dir"
esac
