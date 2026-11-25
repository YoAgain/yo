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
    script=/opt/yo-services/yo/${bot%dev}/${bot%dev}.py  # pingdev: ping on the dev hub
    sed "s#@SCRIPT@#$script#" "$here/script.service" > "$dir/yo-script-$bot@.service"
done
# One book per hub, written by all its executors.
getent group yo-services > /dev/null || groupadd -r yo-services
install -d -m 2770 -g yo-services /var/lib/yo-services
for book in book book-dev; do
    [ -f "/var/lib/yo-services/$book.sqlite" ] || install -m 660 -g yo-services /dev/null "/var/lib/yo-services/$book.sqlite"
done
# Only STATUS sees the numbers it draws.
case " $* " in *" status "*)
    install -d "$dir/yo-script-status@.service.d"
    install -m 644 "$here/status-numbers.conf" "$dir/yo-script-status@.service.d"
    install -m 644 "$here"/yo-status-collect.* "$dir"
esac
