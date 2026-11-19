# skills

Bots as small scripts driving [yo](../app/cli) (Python 3, standard library only): each reads `yo listen` and answers
with `yo send …`, which runs inside the listener. One directory, unit and environment file per bot and hub; the
server switch from `yo-gpt` is in [app/cli/README.md](../app/cli/README.md).

- [yogpt](yogpt): YoGPT, answers every Yo with a Yo or a few, a photo with a reply picture
- [ping](ping): PING, the echo contact: Yos, photos and videos back as they came
- [pingtest](pingtest): PINGTEST, the round-trip probe the apps measure
- [ifttt](ifttt): IFTTT, Yo's service on IFTTT: You Yo IFTTT runs Applets, Yo me lets Applets Yo you
- [status](status): the server's numbers as a picture, sent only to the one user fixed on the server, at the rhythm set on the admin page (Pillow); answers nobody

`yocli.py` holds what they share (running `yo`, the volatile limits); `python3 -m unittest discover skills` checks
their rules, and `cargo test -p yo-cli` runs them end to end against a hub in-process.
