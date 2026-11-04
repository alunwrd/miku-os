# Icons

Drop a picture in here and the build turns it into a QOI at 64 pixels on its
longest side, in `/etc/mayland/icons/<name>.qoi`. The name is the file's, so
`terminal.png` becomes `/etc/mayland/icons/terminal.qoi` — which is the path an
`app { }` block in `/etc/mayland/apps.d/` names.

PNG, JPEG, whatever `image` reads. Transparency is kept: the launcher blends
icons over its own card, so a picture with a background is a picture with a
visible square around it.

Square is what they are drawn as. A picture that is not square is scaled to fit
its longest side and the rest is transparent, so an oblong icon reads as an
oblong icon and not as a stretched one.
