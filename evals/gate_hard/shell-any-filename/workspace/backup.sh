#!/bin/sh
# Copy every .txt file in SRC into DEST.
src=$1
dest=$2
mkdir -p $dest
for f in $(ls $src | grep '\.txt$'); do
  cp $src/$f $dest/
done
