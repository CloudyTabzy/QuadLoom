# Patch layout: QuadLoom vs oracle `20261021-oracle-baseline` (same input: the oracle's `_rem` files)

60 cases with oracle stage files; compared on 60.

| Median | Oracle | QuadLoom |
|---|---|---|
| Patches | 283 | 316 |
| Patch ratio (ours / oracle) | 1 | 1.09 |
| Invalid patches | 0 | 0 |
| Cases with 0 invalid | 47 | 60 |
| Valid % | 100.0 | 100.0 |
| CC-able % (straight sides) | 99.9 | 99.3 |
| Ok % (ours: valid, CC-able, matching singularity) | — | 87.3 |
| Trace time (s) | 1.62 | 0.19 |

QuadLoom's own pipeline (its own repair, features, remesh and field) on 60 inputs: median 204 patches, 59 cases with 0 invalid, 1 invalid in total, Ok 91.2%, 483 seams, median trace 0.29 s.

Totals over the compared cases: oracle 47 invalid (of which 10 deleted non-disk), ours 0 invalid, 439 seams.

| Case | Oracle patches | Ours | Oracle invalid (erased) | Ours invalid | Ours seams | Oracle s | Ours s |
|---|---|---|---|---|---|---|---|
| common3d/armadillo | 969 | 1088 | 0 (0) | 0 | 35 | 10.264 | 0.38 |
| common3d/beetle | 465 | 450 | 0 (0) | 0 | 3 | 1.397 | 0.20 |
| common3d/cheburashka | 162 | 216 | 0 (0) | 0 | 6 | 0.993 | 0.19 |
| common3d/cow | 673 | 704 | 0 (0) | 0 | 4 | 2.188 | 0.17 |
| common3d/fandisk | 29 | 27 | 0 (0) | 0 | 0 | 0.414 | 0.08 |
| common3d/homer | 303 | 330 | 1 (0) | 0 | 11 | 1.374 | 0.19 |
| common3d/igea | 290 | 323 | 0 (0) | 0 | 10 | 3.516 | 0.20 |
| common3d/max-planck | 47 | 88 | 0 (0) | 0 | 0 | 1.541 | 0.46 |
| common3d/rocker-arm | 316 | 408 | 0 (0) | 0 | 11 | 1.486 | 0.17 |
| common3d/stanford-bunny | 529 | 601 | 0 (0) | 0 | 15 | 3.067 | 0.21 |
| common3d/suzanne | 263 | 347 | 0 (0) | 0 | 6 | 1.943 | 0.14 |
| common3d/teapot | 82 | 65 | 0 (0) | 0 | 1 | 0.487 | 0.02 |
| crane/blub | 66 | 65 | 0 (0) | 0 | 0 | 0.858 | 0.39 |
| crane/bob | 23 | 38 | 0 (0) | 0 | 0 | 0.391 | 0.19 |
| crane/crumpleddevelopable | 231 | 257 | 0 (0) | 0 | 0 | 359.999 | 3.23 |
| crane/ogre | 292 | 435 | 0 (0) | 0 | 9 | 2.249 | 0.36 |
| crane/oloid | 438 | 406 | 5 (0) | 0 | 19 | 2.214 | 0.20 |
| crane/pig | 325 | 443 | 1 (0) | 0 | 6 | 1.66 | 0.29 |
| crane/spot | 118 | 184 | 0 (0) | 0 | 4 | 1.095 | 0.13 |
| khronos/AntiqueCamera | 1578 | 1688 | 5 (3) | 0 | 18 | 34.723 | 0.27 |
| khronos/Avocado | 18 | 21 | 0 (0) | 0 | 0 | 0.397 | 0.05 |
| khronos/BarramundiFish | 260 | 252 | 0 (0) | 0 | 2 | 0.852 | 0.08 |
| khronos/BoomBox | 276 | 263 | 0 (0) | 0 | 3 | 0.939 | 0.10 |
| khronos/CesiumMan | 299 | 297 | 0 (0) | 0 | 5 | 0.615 | 0.05 |
| khronos/CesiumMilkTruck | 428 | 420 | 0 (0) | 0 | 10 | 1.881 | 0.03 |
| khronos/CommercialRefrigerator | 1787 | 1730 | 0 (0) | 0 | 9 | 18.335 | 0.22 |
| khronos/Corset | 2170 | 2125 | 4 (0) | 0 | 61 | 9.733 | 0.09 |
| khronos/DamagedHelmet | 995 | 1064 | 2 (0) | 0 | 17 | 4.156 | 0.21 |
| khronos/Duck | 276 | 323 | 0 (0) | 0 | 15 | 1.032 | 0.17 |
| khronos/Fox | 510 | 513 | 0 (0) | 0 | 0 | 3.985 | 0.02 |
| khronos/GlamVelvetSofa | 190 | 177 | 0 (0) | 0 | 3 | 1.137 | 0.05 |
| khronos/Lantern | 453 | 449 | 14 (7) | 0 | 15 | 3.832 | 0.08 |
| khronos/MaterialsVariantsShoe | 348 | 310 | 0 (0) | 0 | 1 | 4.201 | 0.07 |
| khronos/SheenChair | 947 | 869 | 0 (0) | 0 | 19 | 7.489 | 0.06 |
| khronos/SheenCloth | 94 | 154 | 0 (0) | 0 | 2 | 1.571 | 0.40 |
| khronos/SpecularSilkPouf | 3965 | 3568 | 1 (0) | 0 | 67 | 91.613 | 0.81 |
| khronos/SunglassesKhronos | 486 | 513 | 0 (0) | 0 | 3 | 4.145 | 0.08 |
| khronos/WaterBottle | 51 | 75 | 0 (0) | 0 | 0 | 0.454 | 0.06 |
| thingi10k/105762 | 381 | 522 | 0 (0) | 0 | 13 | 2.444 | 0.53 |
| thingi10k/124040 | 68 | 98 | 1 (0) | 0 | 0 | 0.632 | 0.42 |
| thingi10k/1458674 | 260 | 334 | 0 (0) | 0 | 1 | 2.833 | 0.26 |
| thingi10k/1492345 | 84 | 110 | 0 (0) | 0 | 0 | 1.433 | 0.23 |
| thingi10k/1505036 | 292 | 295 | 0 (0) | 0 | 0 | 1.244 | 0.19 |
| thingi10k/1514900 | 955 | 810 | 0 (0) | 0 | 11 | 202.553 | 4.81 |
| thingi10k/1706457 | 106 | 99 | 0 (0) | 0 | 0 | 1.372 | 0.15 |
| thingi10k/250397 | 38 | 29 | 0 (0) | 0 | 0 | 3.823 | 1.61 |
| thingi10k/271869 | 611 | 639 | 1 (0) | 0 | 1 | 9.281 | 0.88 |
| thingi10k/37883 | 142 | 160 | 0 (0) | 0 | 0 | 1.235 | 0.15 |
| thingi10k/40994 | 60 | 88 | 0 (0) | 0 | 0 | 0.654 | 0.16 |
| thingi10k/472206 | 58 | 54 | 0 (0) | 0 | 0 | 0.513 | 0.16 |
| thingi10k/49382 | 12 | 12 | 0 (0) | 0 | 0 | 0.696 | 0.24 |
| thingi10k/62287 | 173 | 187 | 2 (0) | 0 | 0 | 1.503 | 0.20 |
| thingi10k/650187 | 8 | 8 | 0 (0) | 0 | 0 | 16.709 | 4.65 |
| thingi10k/71712 | 384 | 433 | 2 (0) | 0 | 1 | 4.33 | 0.92 |
| thingi10k/75117 | 55 | 70 | 0 (0) | 0 | 0 | 0.942 | 0.13 |
| thingi10k/87355 | 397 | 579 | 0 (0) | 0 | 13 | 3.199 | 0.22 |
| thingi10k/906891 | 620 | 828 | 8 (0) | 0 | 7 | 12.428 | 0.67 |
| thingi10k/94990 | 53 | 62 | 0 (0) | 0 | 0 | 1.244 | 0.15 |
| thingi10k/97730 | 68 | 118 | 0 (0) | 0 | 1 | 0.972 | 0.16 |
| thingi10k/99836 | 355 | 408 | 0 (0) | 0 | 1 | 2.623 | 0.22 |
