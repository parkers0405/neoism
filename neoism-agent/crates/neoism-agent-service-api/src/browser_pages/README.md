# Browser wordmark

`favicon.svg`, `wordmark.svg`, `pixel-wordmark.css`, and the animator bundled in `pixel-wordmark.js` come from the hero in the sibling `neoism-web` project. The animator is unchanged: 15 shuffle-bag variants, one intro, then click/keyboard replay, with reduced-motion and visibility cleanup. These assets are embedded in both browser pages; production does not need the website checkout or a JavaScript build step.

To refresh the compiled animator from the repository root, with the website dependencies already installed:

```sh
node ../neoism-web/node_modules/esbuild/bin/esbuild ../neoism-web/src/pixel-wordmark.ts --bundle --format=iife --global-name=NeoismWordmark --target=es2022 --loader:.css=empty '--banner:js=// Compiled from neoism-web/src/pixel-wordmark.ts; keep all 15 website effects in sync.' '--footer:js=(()=>{const root=document.getElementById("hero-title");if(root)NeoismWordmark.mountPixelWordmark(root)})();' --outfile=neoism-agent/crates/neoism-agent-service-api/src/browser_pages/pixel-wordmark.js
```

Keep the animation stylesheet in sync with `neoism-web/src/pixel-wordmark.css`. The join page authorizes the exact animation and handoff script bytes with separate CSP hashes; do not add `unsafe-inline` or external script sources.

Geist is a WOFF2 conversion of the website's font. Its full SIL Open Font License is retained in `geist-LICENSE.txt` and embedded in the stylesheet.
