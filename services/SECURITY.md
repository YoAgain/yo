# services: security

The design the gatekeeper is built to. A script is taken as hostile, reviewed or not.

| | Gatekeeper | Script |
|-|-|-|
| Account keys, decryption | yes | no |
| Who the sender is | a user id | a handle, the same for every Yo of that sender |
| Network | the hub | none |
| Files | its own state | Python and this repository, read-only |
| Who may be answered, how often | decides (limits are its own, the same for all) | asks |

- `service.toml` is read by the gatekeeper from a read-only checkout; an unknown entry stops the service.
- A photo goes back through a reference; the script never has its key.
- A service whose `name` is not its account's does not start.

A script taken over can still answer those who wrote to it, up to the limits. Not covered: a hole in the
gatekeeper, the CLI or the kernel.
