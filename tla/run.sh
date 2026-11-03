#!/bin/zsh
# usage: ./run.sh <cfg-name-without-extension>   (run from tla/)
JAR="/Applications/TLA+ Toolbox.app/Contents/Eclipse/tla2tools.jar"
HERE="${0:A:h}"
META="${TLC_METADIR:-$(mktemp -d)}"
java -XX:+UseParallelGC -cp "$JAR" tlc2.TLC -deadlock -workers auto \
  -metadir "$META" -config "$HERE/$1.cfg" "$HERE/SpottedArms.tla"
