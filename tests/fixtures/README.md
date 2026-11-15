Keys made for the push tests only (`openssl genpkey`). They sign nothing anywhere else.

`state-1/`: an account of before the keys were libsignal's (commit 1c5d3c2, the last build of state 1 and hub schema 2),
made by a test with throwaway keys on a hub in memory: Jo's sealed state (key `[7; 32]`), Jo's Ed25519 seed, the hub's
database as SQL, and the user ids of Jo and Pal (`ids`). Jo keeps Pal as a friend in the storage (spec 6.9).
