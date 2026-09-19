# P4 downlevel class checks

The native comparison matches all 90 programs and 90 queries in nine focused suites. This closes the explicit private-field downlevel/helper boundary and the computed-static-name boundary; it does not establish whole-corpus acceptance.

The checker keeps private/static-super scope flags and requested helper sets in owner-local link stores. Collision diagnostics run after source checking and retain `skipped_on_no_emit`, which the production diagnostic API filters when `noEmit` is true. Helper module failures are cached per source; successful module lookup checks actual exported value identities and native get/set signature arities.

The compiler already retains synthetic `tslib` resolutions. The new host method supplies the ordinary import mode of that synthetic, attribute-free import. The shared external resolver consumes its text, mode and actual diagnostic node without manufacturing source syntax. Side-effect suppression checks the diagnostic node, matching Go.

The fixture protocol now accepts the explicit pinned target/module enum values and optional `useDefineForClassFields`, `importHelpers`, and `noEmit` booleans. Existing fixture defaults and the independent merge probe remain unchanged.

Validated on the immutable `target/s08/p4-build21507/p2_checker` binary (SHA-256 `408958d8f567475b4b9434668b4840ead3e5aa18a575c310f3940a5221e82ae1`). The build also compiled both P3 examples with `relation-probe` and `ts_checker/storage-pilot`, including the added link-store census.

| Fixture | Programs | Native observation SHA-256 |
| --- | ---: | --- |
| `private-downlevel.json` | 23 | `1a6be914da6041207ee87f742a79a78ed23b4ca93a27ac08206008488a461b53` |
| `private-downlevel-noemit.json` | 23 | `a730e231830289a4062fb3eabdf813df2bd37e98bb93662d169195871343c742` |
| `private-es2022.json` | 10 | `7708d39d07bb95fbed9917f3e61592418307a95d4dab331c699a4be303a1d167` |
| `class-use-define-false.json` | 8 | `864131c2c28ead9ba4014023bb9e4431da2491b2c1f41698824f99eb00266f57` |
| `class-use-define-true.json` | 8 | `19bf26b6bce62ccf522fe0491651752acb40edd4901d65a84d2e7e08da73aac7` |
| `private-import-helpers.json` | 8 | `cec58cbaeba9c83627470c235b815b8e4cb1fe9b64effe9884947002a0ef8adb` |
| `generated-name-collisions.json` | 6 | `4f0d87739e667aef4a149c707f725ea59babb12c8136d70fbfc533a6984eae41` |
| `class-naming-helpers.json` | 2 | `8037eeebd9e91763f9c0d478e24a19637f60744db4aa91eaf78f45de341e0ac3` |
| `private-helpers-node-next.json` | 2 | `50f9306b9a3361a064d17ac562d618830d35f1370fee8c9cbf01194670b4d11f` |

Native archives and comparison records are listed in `target/s08/p4-downlevel-summary.json`. All native captures use pinned Go `1f70213d4922b434345f639b441681e470c7cfc1` and Go 1.27.1 on Darwin arm64. The full reports compare query identities/displays, each diagnostic phase, diagnostic ordering/flags/related information, and rendered error baselines.

The no-emit suite uses the production diagnostic mode; it verifies removal of the deferred generated-name errors while ordinary private-access errors remain. The NodeNext suite loads an actual `node_modules/tslib` package, exercising retained resolution rather than only the ambient-module shortcut.
