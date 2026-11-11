# S06 local snapshot clarification v2 (forward correction)

The preimplementation library-cases.json store-snapshot prose says "during concurrent
committed writer". Python stdlib SQLite 3.45.1 experiment, using the selected DELETE
journal, demonstrated that an active read snapshot prevents another connection's
COMMIT (bounded busy error); it cannot commit concurrently with that snapshot.
The frozen v1 file and its hash remain unchanged. The corrected test expectation is:
"one read sees old head/history while a writer's commit is refused; after reader
exit and writer rollback, a subsequent write commits and the next read sees new
head/history". crates/ont-daemon/tests/restricted.rs implements that narrower expectation.
No CLI oracle output or permission rule changes. This clarification and the full
candidate require independent reviewer/Lead acceptance; author observation is not
independent oracle acceptance. WAL is unnecessary for this S06 single-writer bounded
read workload; reconsider only on measured concurrency need, not by reusing Hindsight.
