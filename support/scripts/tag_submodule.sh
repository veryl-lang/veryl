#!/bin/bash

if [ "$DRY_RUN" = "true" ]; then
    echo "tagging skipped"
else
    sub=("doc" "crates/metadata/std")
    root=`git rev-parse --show-toplevel`

    pushd ${root}

    for dir in ${sub[@]}
    do
        pushd ${dir}
        git tag -f v$1
        git push origin -f v$1
        popd
    done

    popd
fi
