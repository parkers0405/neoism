# Pinned Blaze compatibility patches

These published dependency sources retain their upstream licenses and versions.
They preserve the locale data, encryption, and full-text-search features used by
the services. No feature is disabled to make the application compile.

| Crate | Published archive SHA-256 | Changes |
| --- | --- | --- |
| `icu_collator_data` 2.4.0 | `183f47e821a327661c1ebe152f61d5b6c2000010d228cadbbf226e0e2b8c2634` | Four generated provider macros initialize the same locale fallback object at runtime instead of in a local constant. The pinned compiler cannot normalize the associated data types during constant evaluation. Data bytes and lookup/fallback policy are unchanged. |
| `turso_core` 0.8.3 | `810ee7b139fd6aa9304cb2fa278c1be0bb964c92457e2a24c1d384196ce5e217` | The FTS capture buffer explicitly calls `TursoVecInExt::new_in`. Blaze otherwise selects the unstable inherent `Vec::new_in`, which expects `Global` on the stable compatibility path. Explicit selection preserves Turso's allocator behavior on both configurations. |

The root and native bridge workspaces select the same patches. The Nix source
package includes both patches. The desktop bundle includes both
licenses. `.cargo/config.toml` also selects RustCrypto's supported constant-time
software AES on x86, since Blaze 0.20261029.5 cannot lower `aeskeygenassist` in
either its Cranelift or LLVM backend. A zero-key/zero-block AES-128 known-answer
probe verified the standard ciphertext `66e94bd4ef8a2c3b884cfa59ca342b2e`.

Compatibility probes compiled and ran locale fallback for `en-US`, `fr-CA`,
`sv-SE`, and traditional Spanish, plus a real in-memory Turso FTS index, document
write, matching query, and index optimization. Tantivy uses the unmodified
published crate; its license and authors are retained in `licenses/`.
Turso's license comes from its recorded upstream commit
`607bff9fd0e547a25a97b3bf06e83bcc5d99d53c`.
Production service and database tests remain required; these small probes alone
do not certify the full application.

To remove a patch after upgrading Blaze, first reproduce its corresponding
operation with the unmodified published crate, then run the workspace suite and
installed release checks. Do not replace the data or drop search functionality.
