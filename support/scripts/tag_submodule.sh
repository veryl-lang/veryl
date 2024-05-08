#!/bin/bash

sub=("doc" "std")
root=`git rev-parse --show-toplevel`

pushd ${root}

for dir in ${sub[@]}
do
    pushd ${dir}
    git tag v$1
    git push origin v$1
    popd
done

popd
