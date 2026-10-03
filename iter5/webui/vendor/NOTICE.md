# Vendored browser libraries

Copied unmodified from `~/dev/skillsnap/webapp/node_modules` so `index.html` works offline
from `file://`. Each library's own licence file sits beside it (`LICENSE.<package>`).

| File | Package | Version | Licence |
|---|---|---|---|
| cytoscape.min.js | cytoscape | 3.34.3 | MIT (LICENSE.cytoscape) |
| cytoscape-dagre.js | cytoscape-dagre | 4.0.0 | MIT (LICENSE.cytoscape-dagre) |
| cytoscape-fcose.js | cytoscape-fcose | 2.2.0 | MIT (LICENSE.cytoscape-fcose) |
| cose-base.js | cose-base | 2.2.0 | MIT (LICENSE.cose-base) |
| layout-base.js | layout-base | 2.0.1 | MIT (LICENSE.layout-base) |

The dagre graph-layout library (`@dagrejs/dagre`, MIT licence, Copyright (c) 2012-2014 Chris
Pettitt) is not a separate file: cytoscape-dagre 4.0.0 bundles it inside `cytoscape-dagre.js`,
and no standalone copy of the package exists in that `node_modules` tree to copy a licence
file from.
