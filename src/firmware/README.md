Place the CYW43 firmware blobs here.

Required files (example names):

- 43439A0.bin
- 43439A0_clm.bin

These files must be the correct Broadcom/CYW43 firmware images for your
chipset. The real `network` implementation will expect to load them with
alignment wrappers (e.g., `cyw43::Aligned`).

Do NOT commit large binary firmware files to the repository unless you
intend to track them; instead, place them locally or provide a build step
that downloads the required blobs.
