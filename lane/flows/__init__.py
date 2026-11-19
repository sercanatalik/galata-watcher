"""galata-watcher's lane: cadence and routing, and nothing else.

Served with ``cereyan serve lane``. What it keeps:

1. **Rust decides.** A flow spawns ``galata-watcher-poll`` and turns each
   line it prints into an ``account.<kind>`` event; a rule hands each event
   to ``galata-notify``. Whether something is an alert, and its words, are
   the binaries' business.
2. **The lane holds no credential and passes none.** A job's environment is
   built, never inherited (``_runner.job_env``). ``galata-notify`` reads the
   bot token from its own file.
3. **The run store is deletable.** Every cursor lives on the data volume,
   written by the binaries; deleting cereyan's store changes nothing about
   what the next check finds.
"""
