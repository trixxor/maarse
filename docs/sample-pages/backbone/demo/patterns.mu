@title Pattern book
@bg #f6f7f9
@fg #1b1f24
@link #0969da
@font sans

# Pattern book
Everything a Må:rse page can do, each with the plain text that makes it. Press copy, paste it in your own .mu file, change the words.

{!#e7f6ee:{#11683f: works on LoRa }} {!#e7f6ee:{#11683f: PC and Android alike }} {!#e7f6ee:{#11683f: no HTML, ever }}

[Text](#text) · [Colours](#colours) · [Boxes](#boxes) · [Tables](#tables) · [Layout](#layout) · [Pictures](#pictures) · [The page](#the-page) · [Effects](#effects) · [Widgets](#widgets) · [Locks](#locks) · [Notice board](#notice-board)

::: cards 2
## Text
**bold** · *italic* · ~~struck~~ · `code` · {big:big} · {small:small}
```
**bold** · *italic* · ~~struck~~ · `code`
{big:big} {small:small} {huge:huge}
```
+++
## Colours
{red:red} {orange:orange} {yellow:yellow} {green:green} {blue:blue} {purple:purple} {grey:grey} {#e6007e:any hex}
{!yellow:on yellow} {#fff!#1b5fb8: white on blue }
```
{red:red}  {#e6007e:any hex}
{!yellow:on yellow}  {#fff!#1b5fb8:white on blue}
```
+++
## Boxes
>!green **Good news** in green
>!red [shadow] **Warning** with a hard shadow
```
>!green **Good news** in green
>!red [shadow] **Warning** with a hard shadow
>!#5ad1ff [tilt 1, pulse] any colour, tilted, pulsing
```
+++
## Tables
| Left | Centre | Right |
|---|:---:|---:|
| apples | {green:in stock} | 12 kr |
```
| Left | Centre | Right |
|---|:---:|---:|
| apples | {green:in stock} | 12 kr |
```
+++ 2
## Layout
-> centred <- · lines across: `---` and a double `===` · links to a heading on the same page: `[Tables](#tables)`
```
::: 3            three columns (text flows; one on a phone)
::: 3 dashed     with dashed rules
::: cards 4      a grid of cards; a line of three plus signs starts the next card (with a 2: two wide)
:::              the end of the block
-> centred <-
@dropcap         the next paragraph gets a big first letter
```
+++ 2
## Pictures
![steps: grey first, then colour on a tap](demo/dusk.png "right")
The picture only costs bytes when someone taps it. Over LoRa the smallest copy comes first.
```
![The cabin](hytte.jpg)                 steps from grey to colour
![Chart](chart.png "dots")              dots, sharp for drawings
![Poster](poster.png "colour right")    colour, beside the text
![x](a.jpg "grey! wide tape rec")       forced grey, full width, tape, REC
```
+++ 2
## The page
The whole look comes from a few lines at the top of the .mu file.
```
@bg #0b1220..#0e1726 glow #1f3a66 #3a1f5c    colour, gradient, glow (or: dots)
@fg #e6edf6        text      @link #8fd3ff   links
@font serif        serif, sans or mono
@width 75          reading width in characters
@grid 80x25        a fixed text screen, scaled to fit (BBS)
@ticker text       a running band at the top
```
+++
## Effects
{glow #e6007e:glow} · {gradient #ff3b6b #5ad1ff:gradient} · {outline:outline} · {tilt -4:tilt} · {marker #ffe14d:marker} · {hand #1b5fb8:by hand} · {flash:flash}
```
{glow #e6007e:glow}  {gradient #a #b:…}
{outline:…}  {tilt -4:…}  {marker #ffe14d:…}
{hand #1b5fb8:…}  {stamp red 14:…}  {flash:…}
```
+++
## Widgets
{spark #0969da:3 5 4 8 12 9 14} {ring #11683f:72} {compass:290} {signal:4} {live} {clock}
```
{spark #0969da:3 5 4 8 12 9 14}   {ring #11683f:72}
{compass:290}  {signal:4}  {live}  {clock}
```
+++ 2
## Locks
A page only some people can open; the host keeps the lock and the list never leaves it. See [the crew page](demo/crew).
```
@allow 256665f1e0c94b7d8a1f3c22b9d04e57     one person (their full address)
@allow group crew                          a group in pages/.groups
@allow admins                              the host's administrators
@locked hidden                             others get "Not found"
@locked until 2026-12-24                   open for everyone from that date
```
:::

## Notice board
Readers write here, newest first. Share a tip.
@board 8 table
```
@board 8 table      the 8 newest notes, as a table (or list, or slips)
```

-> [all designs](demo) · [front page](/) <-
