# skills

PING and YoGPT are [services](../services) now; here is what is left.

Bots as small scripts driving [yo](../app/cli) (Python 3, standard library only): each reads `yo listen` and answers
with `yo send …`, which runs inside the listener. One directory, unit and environment file per bot and hub.

- [status](status): the server's numbers as a picture, sent only to the one user fixed on the server, at the rhythm set on the admin page (Pillow); answers nobody

`yocli.py` holds what they share (running `yo`, the volatile limits); `python3 -m unittest discover skills` checks
their rules, and `cargo test -p yo-cli` runs them end to end against a hub in-process.
