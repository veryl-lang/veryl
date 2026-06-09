// Read-only anonymous gateway to the veryl-sccache R2 cache. Fork-PR CI reads
// cached artifacts through here; master writes to R2 directly. Read-only and
// no-listing are enforced below. Rationale: support/cache-worker/README.md.
//
// Both sides use sccache's S3 backend, so a key requested here matches the key
// master wrote.

export default {
  async fetch(request, env) {
    const method = request.method.toUpperCase();
    if (method !== "GET" && method !== "HEAD") {
      return new Response("read-only cache\n", {
        status: 405,
        headers: { allow: "GET, HEAD" },
      });
    }

    const url = new URL(request.url);

    // Refuse listing/enumeration; sccache only ever fetches by exact key.
    if (
      url.pathname === "/" ||
      url.pathname === "" ||
      url.searchParams.has("list-type") ||
      url.searchParams.has("prefix") ||
      url.searchParams.has("delimiter")
    ) {
      return new Response("listing disabled\n", { status: 403 });
    }

    // Path-style S3 requests are /<bucket>/<key>. Drop the bucket segment; the
    // remainder is the R2 object key (e.g. "sccache/<hash>"). The bucket name is
    // irrelevant here — the real bucket is fixed by the binding — so we strip
    // whatever the first segment is rather than matching a fixed name.
    const path = url.pathname.replace(/^\/+/, "");
    const slash = path.indexOf("/");
    if (slash < 0) {
      return new Response("not found\n", { status: 404 }); // bucket only, no key
    }
    const key = decodeURIComponent(path.slice(slash + 1));
    if (!key) {
      return new Response("not found\n", { status: 404 });
    }

    // sccache's S3 backend probes ".sccache_check" at startup. In no-credentials
    // (fork) mode this must not error before any object exists, so answer an
    // absent probe with an empty 200 rather than 404.
    const isProbe = key.endsWith(".sccache_check");

    if (method === "HEAD") {
      const head = await env.CACHE.head(key);
      if (!head) {
        return new Response(null, { status: isProbe ? 200 : 404 });
      }
      const headers = new Headers();
      headers.set("content-length", String(head.size));
      if (head.httpEtag) headers.set("etag", head.httpEtag);
      return new Response(null, { status: 200, headers });
    }

    const obj = await env.CACHE.get(key);
    if (!obj) {
      return isProbe
        ? new Response(null, { status: 200 })
        : new Response("not found\n", { status: 404 });
    }
    const headers = new Headers();
    obj.writeHttpMetadata(headers);
    headers.set("etag", obj.httpEtag);
    return new Response(obj.body, { status: 200, headers });
  },
};
