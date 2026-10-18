# How the intro images were made

These four images were made with Qwen-Image-2.1-Turbo on a local PC. Use this file to make the same images again or to change one image.

## Settings (same for all images)

| Item | Value |
|---|---|
| Model | Qwen-Image-2.1-Turbo, GGUF file `qwen-image-2.1-turbo-Q4_K_M.gguf` (unsloth) |
| Text encoder | `qwen3vl_8b_w4a8.safetensors` |
| VAE | `qwen_image_2.1_vae_bf16.safetensors` |
| Tool | ComfyUI 0.41.0 with the ComfyUI-GGUF node |
| Steps | 8 (custom sigmas from the model card) |
| CFG | 1 |
| Sampler | euler |
| Size | 1152 x 768 pixels |
| Negative prompt | empty |
| After making | Run `finish_image.py` (white background, step number badge, round corners, 256 colors) |

## Style words

Step 1 uses this style text:

> hand-drawn illustration with thin dark navy ink outlines on a clean light background, soft anime-poster color palette: warm orange, soft violet-blue gradients, silver white and pastel pink accents, small sparkle stars, light watercolor glow at the edges, no people, no faces

Steps 2, 3 and 4 add these words at the end of the style text:

> , simple icons, lots of empty space, neat handwritten caption at the bottom

## Images

### step1.png (seed 23)

Prompt = style text for step 1, then:

> A computer monitor seen from the front. The screen is split exactly in two halves by a vertical line. LEFT half: a window with the title bar text 'CrystalDiskMark', below it a large blue rounded badge with the words 'Good' and '100 %', and a small label 'Health Status'. RIGHT half: a window with the title bar text 'Task Manager', below it a red line graph and a big label 'Disk 100%'. Handwritten caption under the monitor: 'PC freezes'.

Note: seeds 5, 11 and 17 gave wrong spelling of the window titles. Seed 23 gave correct spelling.

### step2.png (seed 33)

Prompt = style text for steps 2 to 4, then:

> an SSD drive with a battery icon that is almost empty, old dusty folders next to it, handwritten caption: "Old files lose charge"

### step3.png (seed 33)

> an SSD drive with a magnifying glass over a few folders, small check marks, handwritten caption: "Scan reads slow files"

### step4.png (seed 33)

> a PC monitor with a fast speed gauge pointing to high and a green check mark, handwritten caption: "Fast again"

## Speed on the test PC

RTX 4070 Laptop GPU with 8 GB VRAM. One image takes about 25 to 33 seconds when the models are loaded. Peak VRAM use is 6.8 GB.

## Change one image

1. Start ComfyUI with the Qwen-Image-2.1-Turbo workflow above.
2. Change the words in the prompt. Keep the same style text.
3. Try 4 seeds. Pick the image with correct spelling in all words.
4. Run `uv run --with pillow --with numpy python finish_image.py RAW.png STEP_NUMBER stepN.png` on the raw PNG.
5. Add a line below to say what changed.

## Change log

- 2026-10-18: first version of the four images.
- 2026-10-18: all backgrounds pure white, step number badges, round corners and no border line.
