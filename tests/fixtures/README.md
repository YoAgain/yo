Keys made for the push tests only (`openssl genpkey`). They sign nothing anywhere else.

`state-1/`: an account of before the keys were libsignal's (commit 1c5d3c2, the last build of state 1 and hub schema 2),
made by a test with throwaway keys on a hub in memory: Jo's sealed state (key `[7; 32]`), Jo's Ed25519 seed, the hub's
database as SQL, and the user ids of Jo and Pal (`ids`). Jo keeps Pal as a friend in the storage (spec 6.9).

`keychain-v1/` contains synthetic items for the unversioned Keychain format and version 3. The state-v2
blob uses the same synthetic account as `state-1/`, with its converted Curve25519 account secret. The
SQL fixtures provide the single-row schema and the encrypted account column from the old tables schema. Both
converted forms keep the key the storage of then is sealed under (`legacy_storage_key`), as a converted state did.
The migration test uses the v1 blob directly as well as in the App Group's prefixed flat-file format.
