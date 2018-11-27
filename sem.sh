mkdir ../cache
git status
rm Cargo.lock
ls -a
./src/ci/init_repo.sh . ../cache
#git submodule sync
#git submodule update -j 16 --init --recursive
#ls -a -R src/tools/clippy