# Trust store

`ca-bundle.pem` is the Mozilla CA list as shipped by `ca-certificates`
(121 roots). It lands on the machine as `/etc/ssl/certs/ca-bundle.pem`, and
`lib/net` parses it at run time - `verify::system_trust()`.

It is a file, and it used to be compiled into the kernel. That was the right
answer while TLS lived in ring 0: a kernel booted with `qemu -kernel` has no
filesystem, and a trust store missing on some boots turns "this does not chain
to anything I trust" into an accident of how the machine started.

Out here the guarantee is kept a different way. `/etc` is part of the boot
medium - the GRUB module the kernel came from, mounted read-only before
anything runs - so on any ordinary boot the file is there. And when it is not,
nothing guesses: a program that cannot read it refuses to connect and says
why. `--insecure` is the only way past, and it prints a warning on every
connection made under it.

That is the trade the file buys: a developer can see what the machine trusts,
add a root of their own for a server on the bench, and take one out - none of
which was possible when the answer was baked into a kernel image.

To update it, replace the file. Nothing needs regenerating; the tests in
`lib/net/tests/trust.rs` read this same file and will say if it stops parsing.
