# Flow review browser

Requires Node.js 22.12 or later (Node 22 recommended). Dependencies are exact
pins; use `npm ci`. The server serves `dist/` and the same-origin API.

```sh
npm ci
npm run lint
npm test
npm run build
```

`npm run build` includes TypeScript checking. `npm run check` runs that check
alone. `npm run dev` starts Vite on localhost and proxies `/api` to localhost:8080.
No package data is sent to a third-party renderer or telemetry service.

Browser tests exercise the real UI with a mocked inspection API (separate server
checks establish real Rust validation). With an existing Chromium executable:

```sh
PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/usr/bin/chromium npm run test:browser
```

Alternatively install Playwright Chromium explicitly and set
`PLAYWRIGHT_BROWSERS_PATH` consistently for install and tests. Tests start the
Vite server on localhost:4173, or use `FLOW_REVIEW_BROWSER_URL` for an existing
server. They cover import, instructions, feedback/download, comparisons and
incomplete packages. Logic tests include a generated 310 KB / 1500-node cycle.

For the maintained test against the actual Rust server, run that server with
the built browser assets and the repository's `flows/starters` mounted as its
package directory, then run:

```sh
FLOW_REVIEW_REAL_API_URL=http://127.0.0.1:8080 \
FLOW_REVIEW_BROWSER_URL=http://127.0.0.1:8080 \
PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/usr/bin/chromium \
npm run test:browser -- --grep 'real API'
```

This test checks mounted source, actual bundle upload, prompts/graph, feedback
download, revision comparison and served dependency notices. It is explicitly
skipped without `FLOW_REVIEW_REAL_API_URL`; fixture browser tests are separate.

The build includes complete upstream license texts from the installed production
lock tree in `dist/third-party-notices.txt`. `scripts/build-notices.mjs` fails if
an installed version differs from the lock or a license text is missing. Keep
this generated asset with the browser distribution; it is linked in the footer.

Comments and the comparison baseline live only in page memory. Export Markdown
before closing or refreshing the browser. Loading another package preserves
comments with their original revision; it does not resolve them automatically.
Use current as baseline, load the next revision, then inspect concrete changes.
Renames appear as removal/addition, and source text changes do not establish
semantic equivalence. Source files are never edited by the browser.

Folder imports strip the browser's common selected root folder; text-file
imports preserve supplied filenames. Root/registry selectors rerun inspection
from the returned source snapshot. Unknown inspection versions are rejected.
Graph and node list initially show 100 matching nodes; expand incrementally.
All nodes remain available, including unassigned and unreachable nodes.
