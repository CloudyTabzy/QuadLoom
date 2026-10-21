# Features: QuadLoom `20261021-m3a` vs oracle `20261021-oracle-baseline`

70 cases, 60 comparable. Median length-ratio quotient (ours / oracle): **0.890**

| Case | Our edges | Our corners | Our L/√A | Oracle corners | Oracle L/√A | Quotient | Cleanup |
|---|---|---|---|---|---|---|---|
| common3d__alligator | 433 | 0 | 9.55 | — | — | — | — |
| common3d__armadillo | 680 | 155 | 2.45 | 746 | 3.57 | 0.69 | — |
| common3d__beetle | 843 | 220 | 30.16 | 342 | 32.37 | 0.93 | split_non_manifold_edges=47, split_non_manifold_vertices=48, flipped_faces=1, skipped_small_components=10, skipped_small_component_faces=16 |
| common3d__cheburashka | 298 | 24 | 3.56 | 101 | 3.95 | 0.90 | — |
| common3d__cow | 831 | 244 | 13.66 | 599 | 18.30 | 0.75 | split_non_manifold_vertices=1 |
| common3d__fandisk | 716 | 24 | 8.91 | 24 | 8.90 | 1.00 | — |
| common3d__homer | 127 | 31 | 0.83 | 261 | 2.22 | 0.37 | — |
| common3d__igea | 379 | 87 | 1.14 | 205 | 1.30 | 0.88 | — |
| common3d__max-planck | 186 | 2 | 0.77 | 2 | 0.64 | 1.20 | — |
| common3d__rocker-arm | 281 | 73 | 1.69 | 331 | 3.07 | 0.55 | — |
| common3d__stanford-bunny | 693 | 110 | 4.07 | 383 | 5.49 | 0.74 | — |
| common3d__suzanne | 530 | 238 | 23.27 | 256 | 21.04 | 1.11 | welded_vertices=2, removed_duplicates=1, split_non_manifold_edges=1, split_non_manifold_vertices=5, skipped_small_components=1, skipped_small_component_faces=1 |
| common3d__teapot | 297 | 6 | 5.00 | 41 | 20.95 | 0.24 | welded_vertices=403, split_non_manifold_vertices=1 |
| common3d__woody | 119 | 0 | 5.83 | — | — | — | — |
| crane__blub | 115 | 14 | 2.02 | 30 | 2.34 | 0.86 | — |
| crane__bob | 0 | 0 | -0.00 | 4 | 0.05 | -0.00 | — |
| crane__crumpleddevelopable | 372 | 166 | 26.40 | 177 | 26.69 | 0.99 | — |
| crane__ogre | 508 | 72 | 5.39 | 251 | 6.66 | 0.81 | — |
| crane__oloid | 3542 | 46 | 37.48 | 59 | 4.37 | 8.57 | welded_vertices=2, removed_repeated=4 |
| crane__pig | 415 | 94 | 4.99 | 230 | 6.26 | 0.80 | welded_vertices=40 |
| crane__spot | 120 | 34 | 1.93 | 74 | 2.44 | 0.79 | — |
| khronos__ABeautifulGame | 17832 | 642 | 17.99 | — | — | — | welded_vertices=194940, removed_zero_area=536, removed_duplicates=932, split_non_manifold_edges=40, split_non_manifold_vertices=120, flipped_faces=2368, skipped_small_components=80, skipped_small_component_faces=144 |
| khronos__AntiqueCamera | 8565 | 1353 | 85.90 | 86 | 186.30 | 0.46 | welded_vertices=10971, split_non_manifold_edges=83, split_non_manifold_vertices=87, flipped_faces=36, skipped_small_components=2, skipped_small_component_faces=2 |
| khronos__Avocado | 89 | 8 | 3.36 | 16 | 5.73 | 0.59 | welded_vertices=43 |
| khronos__BarramundiFish | 577 | 113 | 8.72 | 238 | 17.12 | 0.51 | welded_vertices=250 |
| khronos__BoomBox | 764 | 178 | 10.11 | 260 | 17.26 | 0.59 | welded_vertices=442 |
| khronos__BrainStem | 22963 | 4678 | 115.56 | — | — | — | welded_vertices=1600, split_non_manifold_edges=72, split_non_manifold_vertices=156, flipped_faces=16, skipped_small_components=24, skipped_small_component_faces=51 |
| khronos__CesiumMan | 684 | 58 | 17.74 | 126 | 33.19 | 0.53 | welded_vertices=935 |
| khronos__CesiumMilkTruck | 1236 | 380 | 32.55 | 36 | 86.41 | 0.38 | welded_vertices=3371, removed_duplicates=768, split_non_manifold_vertices=2 |
| khronos__ChronographWatch | 22616 | 5141 | 77.46 | — | — | — | welded_vertices=54978, removed_repeated=18, removed_zero_area=49, removed_duplicates=716, split_non_manifold_edges=798, split_non_manifold_vertices=1113, flipped_faces=250, cut_edges=47, skipped_small_components=128, skipped_small_component_faces=191 |
| khronos__CommercialRefrigerator | 8605 | 1764 | 142.40 | 1422 | 204.13 | 0.70 | welded_vertices=8836, skipped_small_components=3, skipped_small_component_faces=10 |
| khronos__Corset | 9269 | 2655 | 36.84 | 3068 | 48.46 | 0.76 | welded_vertices=1731 |
| khronos__DamagedHelmet | 5251 | 765 | 62.06 | 0 | 118.74 | 0.52 | welded_vertices=5895, split_non_manifold_edges=7, split_non_manifold_vertices=19, skipped_small_components=5, skipped_small_component_faces=5 |
| khronos__DragonAttenuation | 12110 | 3707 | 16.58 | — | — | — | welded_vertices=71573, removed_duplicates=5, split_non_manifold_edges=6, split_non_manifold_vertices=13, skipped_small_components=4, skipped_small_component_faces=4 |
| khronos__Duck | 232 | 40 | 3.26 | 136 | 12.26 | 0.27 | welded_vertices=291 |
| khronos__Fox | 252 | 104 | 18.62 | 0 | 108.89 | 0.17 | welded_vertices=1438 |
| khronos__GlamVelvetSofa | 926 | 81 | 14.43 | 167 | 37.02 | 0.39 | welded_vertices=955 |
| khronos__Lantern | 1588 | 160 | 30.89 | 224 | 63.19 | 0.49 | welded_vertices=1411, removed_repeated=6 |
| khronos__MaterialsVariantsShoe | 2632 | 178 | 24.08 | 316 | 48.71 | 0.49 | welded_vertices=1608, split_non_manifold_vertices=2 |
| khronos__SciFiHelmet | 7789 | 1827 | 68.76 | — | — | — | welded_vertices=58195 |
| khronos__SheenChair | 2522 | 392 | 14.92 | 554 | 35.73 | 0.42 | welded_vertices=2004 |
| khronos__SheenCloth | 1420 | 92 | 6.02 | 138 | 5.80 | 1.04 | — |
| khronos__SpecularSilkPouf | 17673 | 2596 | 94.57 | 3640 | 100.06 | 0.95 | welded_vertices=689 |
| khronos__SunglassesKhronos | 3422 | 362 | 47.56 | 395 | 56.75 | 0.84 | welded_vertices=624, removed_duplicates=4, split_non_manifold_edges=214, split_non_manifold_vertices=404, flipped_faces=32, skipped_small_components=88, skipped_small_component_faces=176 |
| khronos__ToyCar | 9885 | 494 | 23.64 | — | — | — | welded_vertices=21037, split_non_manifold_vertices=12 |
| khronos__TrafficCone | 542 | 44 | 5.92 | — | — | — | welded_vertices=1842, removed_repeated=16 |
| khronos__WaterBottle | 672 | 40 | 14.86 | 56 | 20.80 | 0.71 | welded_vertices=164 |
| thingi10k__105762 | 13162 | 336 | 74.75 | 336 | 74.25 | 1.01 | — |
| thingi10k__124040 | 896 | 6 | 9.40 | 6 | 9.39 | 1.00 | — |
| thingi10k__1458674 | 1589 | 102 | 15.98 | 102 | 15.96 | 1.00 | — |
| thingi10k__1492345 | 1060 | 26 | 7.16 | 28 | 7.24 | 0.99 | welded_vertices=2, removed_repeated=4 |
| thingi10k__1505036 | 2151 | 402 | 27.89 | 288 | 24.90 | 1.12 | welded_vertices=59, removed_repeated=118 |
| thingi10k__1514900 | 3621 | 196 | 42.60 | 712 | 31.10 | 1.37 | — |
| thingi10k__1706457 | 591 | 25 | 9.47 | 26 | 9.34 | 1.01 | — |
| thingi10k__250397 | 316 | 62 | 11.23 | 32 | 4.76 | 2.36 | welded_vertices=6, removed_repeated=12 |
| thingi10k__271869 | 2880 | 544 | 36.15 | 546 | 35.77 | 1.01 | — |
| thingi10k__37883 | 1685 | 112 | 16.39 | 80 | 16.56 | 0.99 | — |
| thingi10k__40994 | 720 | 16 | 4.39 | 22 | 4.43 | 0.99 | — |
| thingi10k__472206 | 2460 | 24 | 11.27 | 24 | 11.27 | 1.00 | — |
| thingi10k__49382 | 229 | 0 | 2.26 | 0 | 2.26 | 1.00 | — |
| thingi10k__62287 | 462 | 74 | 11.91 | 115 | 12.42 | 0.96 | — |
| thingi10k__650187 | 3962 | 0 | 6.43 | 0 | 6.43 | 1.00 | welded_vertices=2, removed_repeated=4 |
| thingi10k__71712 | 2298 | 288 | 27.92 | 293 | 27.98 | 1.00 | — |
| thingi10k__75117 | 970 | 20 | 11.71 | 20 | 11.70 | 1.00 | — |
| thingi10k__756803 | 10392 | 5760 | 102.87 | — | — | — | — |
| thingi10k__87355 | 415 | 80 | 3.43 | 425 | 5.63 | 0.61 | — |
| thingi10k__906891 | 7765 | 138 | 11.65 | 171 | 11.51 | 1.01 | welded_vertices=8, removed_repeated=16 |
| thingi10k__94990 | 1149 | 46 | 9.95 | 40 | 9.74 | 1.02 | — |
| thingi10k__97730 | 391 | 28 | 7.00 | 22 | 6.93 | 1.01 | — |
| thingi10k__99836 | 875 | 354 | 23.91 | 346 | 23.62 | 1.01 | — |
