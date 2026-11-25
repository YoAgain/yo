# services

The gatekeeper: runs bots. **A shell so far, no code yet**; the live bots run from [skills](../skills).

A bot is a Yo account and a small script from [yo-services](https://github.com/YoAgain/yo-services). The gatekeeper
stands between the two, one process per bot, an ordinary user to the hub:

```text
Hub <-> gatekeeper (account, keys) <-> script (no keys, no network)
```

- **In:** it decrypts what comes and tells the script no more than "a Yo from 7f3a".
- **Out:** the script asks to answer 7f3a; it checks rights and limits (`service.toml`) and sends.
- **Addresses:** a script never reaches the internet itself; the gatekeeper calls only those listed in `service.toml`.

Planned, in Rust:

```text
services/
├─ src/main.rs       one bot: account, script, limits
├─ src/policy.rs     service.toml, what is not there is not allowed
├─ src/script.rs     the lines to and from the script
└─ systemd/          the sandbox, a user of its own for the script
```
