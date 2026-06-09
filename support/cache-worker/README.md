# veryl-sccache cache worker

Read-only, anonymous gateway to the `veryl-sccache` Cloudflare R2 bucket.

## Why

CI build cache lives in R2 (`veryl-sccache`). Trusted **master** builds write to
it directly over the R2 S3 API (credentials in GitHub secrets). **Fork PRs**
receive no secrets and R2's S3 API has no anonymous access, so they cannot read
the cache directly. This Worker exposes a minimal **read-only** S3-style `GET`
over the bucket (via an R2 binding) so fork PRs can warm-read the cache.

- Writes are impossible here (no `PUT`/`DELETE`). Read-only is enforced in
  `src/index.js`.
- Listing/enumeration is refused; objects are only fetched by exact key.

## Deploy (manual)

```bash
cd support/cache-worker
npm install
npx wrangler login        # one-time, OAuth to the Cloudflare account
npx wrangler deploy
```

`wrangler deploy` prints the public URL, e.g.
`https://veryl-sccache.<your-subdomain>.workers.dev`.

**Report that URL back** — it goes into the CI workflow as the fork read
endpoint (`SCCACHE_ENDPOINT` for fork PRs, with `SCCACHE_S3_NO_CREDENTIALS=true`).
The URL is public (not a secret).

Rollback if needed: `npx wrangler rollback` (or the Cloudflare dashboard version
history).

## Notes

- The R2 binding (`CACHE` → `veryl-sccache`) is declared in `wrangler.toml`;
  the Worker needs no secrets to read.
- Account id is in `wrangler.toml` (not sensitive).
- The exact sccache-S3-no-credentials request shape against this endpoint will
  be validated in the CI pilot; the key-derivation logic may be tweaked then.
