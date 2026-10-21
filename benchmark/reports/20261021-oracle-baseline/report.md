# Benchmark report — `20261021-oracle-baseline` (oracle)

Generated 2026-10-21 21:52. 70 cases: **4** error, **60** ok, **6** timeout. Pure-quad results: 60/60.

## Summary (medians over completed cases)

| Metric | Value |
|---|---|
| Mean scaled Jacobian (primary) | 0.9222 |
| Singularities / 1k quads (primary) | 22.38 |
| Min scaled Jacobian (guard) | -0.9979 |
| Mean distance, % diag (guard) | 0.0367 |
| Hausdorff, % diag | 0.491 |
| Mean angle deviation, deg (guard) | 13.75 |
| Tool time, s | 12.95 |

## Cases

| Case | Status | Time s | Faces in | Quads out | Pure | Sing/1k | SJ mean | SJ min | Dist % | Hausdorff % |
|---|---|---|---|---|---|---|---|---|---|---|
| common3d/alligator | error | — | 5981 | — | — | — | — | — | — | — |
| common3d/armadillo | ok | 89.2 | 99976 | 23399 | yes | 34.6 | 0.875 | -1.000 | 0.0625 | 0.453 |
| common3d/beetle | ok | 11.3 | 2053 | 10370 | yes | 54.5 | 0.836 | -0.673 | 0.0178 | 0.745 |
| common3d/cheburashka | ok | 9.0 | 13334 | 6471 | yes | 21.3 | 0.896 | -0.829 | 0.0644 | 0.668 |
| common3d/cow | ok | 11.7 | 5804 | 11409 | yes | 44.0 | 0.863 | -0.881 | 0.0282 | 0.430 |
| common3d/fandisk | ok | 4.1 | 12946 | 4542 | yes | 7.5 | 0.966 | 0.358 | 0.0112 | 0.580 |
| common3d/homer | ok | 11.6 | 12000 | 9109 | yes | 27.6 | 0.848 | -0.993 | 0.0508 | 0.712 |
| common3d/igea | ok | 37.1 | 268686 | 10236 | yes | 25.3 | 0.886 | -1.000 | 0.0542 | 0.469 |
| common3d/max-planck | ok | 19.6 | 99991 | 7165 | yes | 5.6 | 0.940 | -0.421 | 0.0711 | 0.818 |
| common3d/rocker-arm | ok | 12.0 | 20088 | 10813 | yes | 22.7 | 0.878 | -0.998 | 0.0662 | 0.572 |
| common3d/stanford-bunny | ok | 31.3 | 69451 | 12805 | yes | 35.8 | 0.873 | -1.000 | 0.0540 | 0.407 |
| common3d/suzanne | ok | 9.3 | 500 | 6458 | yes | 27.9 | 0.914 | -0.558 | 0.0199 | 0.956 |
| common3d/teapot | ok | 7.9 | 6320 | 5180 | yes | 20.3 | 0.972 | -0.458 | 0.0477 | 0.394 |
| common3d/woody | error | — | 1267 | — | — | — | — | — | — | — |
| crane/blub | ok | 9.3 | 14208 | 5659 | yes | 11.5 | 0.924 | 0.007 | 0.0463 | 0.427 |
| crane/bob | ok | 5.4 | 10688 | 4495 | yes | 4.4 | 0.970 | 0.707 | 0.0408 | 0.645 |
| crane/crumpleddevelopable | ok | 519.3 | 512 | 122220 | yes | 2.3 | 0.893 | -0.303 | 0.0010 | 0.102 |
| crane/ogre | ok | 21.7 | 39856 | 11449 | yes | 21.2 | 0.878 | -1.000 | 0.0699 | 0.806 |
| crane/oloid | ok | 212.0 | 8192 | 12493 | yes | 47.4 | 0.782 | -1.000 | 0.0155 | 0.407 |
| crane/pig | ok | 12.4 | 11392 | 9583 | yes | 28.0 | 0.873 | -1.000 | 0.0617 | 0.549 |
| crane/spot | ok | 7.9 | 5856 | 5809 | yes | 13.4 | 0.929 | -1.000 | 0.0468 | 0.532 |
| khronos/ABeautifulGame | timeout | — | — | — | — | — | — | — | — | — |
| khronos/AntiqueCamera | ok | 682.9 | 20066 | 90887 | yes | 31.1 | 0.935 | -1.000 | 0.0096 | 0.611 |
| khronos/Avocado | ok | 4.7 | 682 | 4892 | yes | 3.1 | 0.968 | -0.057 | 0.0185 | 0.320 |
| khronos/BarramundiFish | ok | 7.9 | 3864 | 8351 | yes | 32.8 | 0.884 | -0.954 | 0.0190 | 0.252 |
| khronos/BoomBox | ok | 8.8 | 6036 | 8083 | yes | 25.5 | 0.939 | -1.000 | 0.0354 | 1.244 |
| khronos/BrainStem | timeout | — | — | — | — | — | — | — | — | — |
| khronos/CesiumMan | ok | 8.1 | 4672 | 6971 | yes | 57.8 | 0.920 | -0.382 | 0.0207 | 1.189 |
| khronos/CesiumMilkTruck | ok | 22.8 | 3624 | 12078 | yes | 60.7 | 0.886 | -1.000 | 0.0556 | 3.400 |
| khronos/ChronographWatch | timeout | — | — | — | — | — | — | — | — | — |
| khronos/CommercialRefrigerator | ok | 545.9 | 208679 | 67420 | yes | 27.0 | 0.913 | -1.000 | 0.0966 | 0.729 |
| khronos/Corset | ok | 129.1 | 18324 | 48059 | yes | 19.6 | 0.938 | -1.000 | 0.0141 | 0.406 |
| khronos/DamagedHelmet | ok | 85.9 | 15452 | 19026 | yes | 61.5 | 0.898 | -1.000 | 0.0434 | 2.854 |
| khronos/DragonAttenuation | timeout | — | — | — | — | — | — | — | — | — |
| khronos/Duck | ok | 10.0 | 4212 | 6927 | yes | 30.5 | 0.907 | -0.624 | 0.0370 | 0.407 |
| khronos/Fox | ok | 26.7 | 576 | 12331 | yes | 87.7 | 0.757 | -1.000 | 0.0088 | 2.071 |
| khronos/GlamVelvetSofa | ok | 13.0 | 4196 | 10311 | yes | 16.4 | 0.900 | -1.000 | 0.0288 | 0.636 |
| khronos/Lantern | ok | 43.1 | 5394 | 18024 | yes | 35.3 | 0.915 | -1.000 | 0.0142 | 0.522 |
| khronos/MaterialsVariantsShoe | ok | 54.3 | 22700 | 18883 | yes | 12.7 | 0.935 | -1.000 | 0.0331 | 0.626 |
| khronos/SciFiHelmet | error | — | 23358 | — | — | — | — | — | — | — |
| khronos/SheenChair | ok | 100.0 | 39936 | 26652 | yes | 41.7 | 0.681 | -1.000 | 0.0569 | 0.531 |
| khronos/SheenCloth | ok | 35.8 | 115200 | 7000 | yes | 4.6 | 0.931 | -0.287 | 0.0938 | 1.365 |
| khronos/SpecularSilkPouf | ok | 565.0 | 81280 | 100790 | yes | 45.3 | 0.739 | -1.000 | 0.0263 | 0.377 |
| khronos/SunglassesKhronos | ok | 35.2 | 13396 | 20180 | yes | 20.1 | 0.889 | -1.000 | 0.0182 | 0.747 |
| khronos/ToyCar | error | — | 108936 | — | — | — | — | — | — | — |
| khronos/TrafficCone | timeout | — | — | — | — | — | — | — | — | — |
| khronos/WaterBottle | ok | 6.9 | 4510 | 6895 | yes | 5.4 | 0.959 | -1.000 | 0.0663 | 1.325 |
| thingi10k/105762 | ok | 20.9 | 30572 | 18468 | yes | 19.3 | 0.880 | -0.990 | 0.0095 | 0.208 |
| thingi10k/124040 | ok | 5.5 | 7384 | 5987 | yes | 11.0 | 0.953 | 0.168 | 0.0250 | 0.406 |
| thingi10k/1458674 | ok | 13.3 | 4438 | 8968 | yes | 27.9 | 0.950 | -0.234 | 0.0365 | 0.491 |
| thingi10k/1492345 | ok | 13.3 | 10424 | 8854 | yes | 9.8 | 0.970 | -0.986 | 0.0393 | 0.244 |
| thingi10k/1505036 | ok | 10.5 | 4410 | 8231 | yes | 43.7 | 0.952 | -0.999 | 0.0133 | 0.274 |
| thingi10k/1514900 | ok | 544.8 | 7066 | 88716 | yes | 11.8 | 0.885 | -1.000 | 0.0078 | 0.309 |
| thingi10k/1706457 | ok | 8.7 | 17748 | 6426 | yes | 15.6 | 0.958 | 0.323 | 0.0890 | 1.931 |
| thingi10k/250397 | ok | 24.4 | 3424 | 20241 | yes | 0.5 | 0.996 | -0.000 | 0.0103 | 0.150 |
| thingi10k/271869 | ok | 36.3 | 6648 | 27714 | yes | 22.2 | 0.920 | -1.000 | 0.0101 | 0.409 |
| thingi10k/37883 | ok | 8.5 | 3302 | 6548 | yes | 22.6 | 0.942 | -0.740 | 0.0437 | 0.904 |
| thingi10k/40994 | ok | 7.2 | 34984 | 5130 | yes | 9.7 | 0.954 | -0.824 | 0.0545 | 0.491 |
| thingi10k/472206 | ok | 8.9 | 32912 | 7943 | yes | 6.0 | 0.991 | 0.324 | 0.0629 | 0.213 |
| thingi10k/49382 | ok | 6.7 | 9190 | 4402 | yes | 1.8 | 0.992 | 0.797 | 0.0455 | 0.150 |
| thingi10k/62287 | ok | 9.0 | 3368 | 8529 | yes | 23.0 | 0.965 | -1.000 | 0.0311 | 0.450 |
| thingi10k/650187 | ok | 99.3 | 25400 | 52437 | yes | 0.1 | 0.930 | 0.205 | 0.0033 | 0.057 |
| thingi10k/71712 | ok | 34.1 | 4488 | 19378 | yes | 24.2 | 0.952 | -0.998 | 0.0027 | 0.130 |
| thingi10k/75117 | ok | 6.9 | 9094 | 5500 | yes | 7.6 | 0.939 | -0.100 | 0.0289 | 0.143 |
| thingi10k/756803 | timeout | — | — | — | — | — | — | — | — | — |
| thingi10k/87355 | ok | 23.7 | 22752 | 11381 | yes | 30.2 | 0.882 | -1.000 | 0.0481 | 0.473 |
| thingi10k/906891 | ok | 54.1 | 41300 | 25005 | yes | 25.9 | 0.900 | -1.000 | 0.0524 | 1.306 |
| thingi10k/94990 | ok | 9.7 | 27200 | 7256 | yes | 6.6 | 0.951 | -1.000 | 0.0454 | 1.153 |
| thingi10k/97730 | ok | 6.5 | 3110 | 5230 | yes | 13.8 | 0.963 | -0.984 | 0.0522 | 0.407 |
| thingi10k/99836 | ok | 12.9 | 3136 | 9986 | yes | 40.5 | 0.946 | -1.000 | 0.0124 | 0.219 |

## Failures

- **common3d/alligator** — error: RuntimeError: Error: Remeshing failed: remeshAndField failed

- **common3d/woody** — error: RuntimeError: Error: Remeshing failed: remeshAndField failed

- **khronos/ABeautifulGame** — timeout: no result after 900s
- **khronos/BrainStem** — timeout: no result after 900s
- **khronos/ChronographWatch** — timeout: no result after 900s
- **khronos/DragonAttenuation** — timeout: no result after 900s
- **khronos/SciFiHelmet** — error: RuntimeError: Error: Remeshing failed: remeshAndField failed

- **khronos/ToyCar** — error: RuntimeError: Error: Remeshing failed: quadPatches failed

- **khronos/TrafficCone** — timeout: no result after 900s
- **thingi10k/756803** — timeout: no result after 900s
