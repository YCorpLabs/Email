#!/usr/bin/env bash
# Only for a stopped, disposable Ubuntu cloud-image QA VM. Keeps its original disk.
set -euo pipefail
if [[ $# != 2 ]]; then
  echo "Usage: $0 /path/to/disk.qcow2 /path/to/seed-directory" >&2
  exit 2
fi
disk_dir=$(cd "$(dirname "$1")" && pwd)
disk_name=$(basename "$1")
seed_dir=$(cd "$2" && pwd)
[[ -f "$disk_dir/$disk_name" && -f "$seed_dir/user-data" && -f "$seed_dir/meta-data" ]]
backup="$disk_dir/$disk_name.before-nocloud-seed"
[[ ! -e "$backup" ]] || { echo "Backup already exists; refusing to replace it." >&2; exit 1; }
# No mail or host account credentials are mounted. The seed contains only the VM login.
docker run --rm -v "$disk_dir:/disk" -v "$seed_dir:/seed:ro" -e DISK_NAME="$disk_name" ubuntu:24.04 bash -euc '
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq >/dev/null
  apt-get install -y -qq qemu-utils e2fsprogs fdisk python3 >/dev/null
  work=$(mktemp -d /disk/.nocloud.XXXXXX)
  trap '\''rm -rf "$work"'\'' EXIT
  qemu-img convert -f qcow2 -O raw "/disk/$DISK_NAME" "$work/full.raw"
  sfdisk --json "$work/full.raw" 2>/dev/null > "$work/partitions.json"
  read -r start size < <(python3 -c '\''import json,sys; p=max((p for p in json.load(open(sys.argv[1]))["partitiontable"]["partitions"] if p["type"].lower()=="0fc63daf-8483-4772-8e79-3d69d8477de4"),key=lambda p:p["size"]); print(p["start"],p["size"])'\'' "$work/partitions.json")
  dd if="$work/full.raw" of="$work/root.raw" bs=4M skip="$((start * 512))" count="$((size * 512))" iflag=skip_bytes,count_bytes status=none
  e2fsck -fy "$work/root.raw" >/dev/null 2>&1 || [[ $? == 1 ]]
  debugfs -R stats "$work/root.raw" 2>/dev/null | grep -q cloudimg-rootfs
  for dir in /var/lib/cloud /var/lib/cloud/seed /var/lib/cloud/seed/nocloud; do
    debugfs -w -R "mkdir $dir" "$work/root.raw" >/dev/null 2>&1
  done
  for file in user-data meta-data; do
    debugfs -w -R "write /seed/$file /var/lib/cloud/seed/nocloud/$file" "$work/root.raw" >/dev/null 2>&1
    debugfs -w -R "set_inode_field /var/lib/cloud/seed/nocloud/$file mode 0100600" "$work/root.raw" >/dev/null 2>&1
    debugfs -R "cat /var/lib/cloud/seed/nocloud/$file" "$work/root.raw" 2>/dev/null | cmp -s - "/seed/$file"
  done
  dd if="$work/root.raw" of="$work/full.raw" bs=4M seek="$((start * 512))" oflag=seek_bytes conv=notrunc status=none
  qemu-img convert -f raw -O qcow2 "$work/full.raw" "/disk/$DISK_NAME.nocloud-new"
  qemu-img check "/disk/$DISK_NAME.nocloud-new"
'
mv "$disk_dir/$disk_name" "$backup"
mv "$disk_dir/$disk_name.nocloud-new" "$disk_dir/$disk_name"
chmod 600 "$disk_dir/$disk_name" "$backup"
echo "Prepared deterministic NoCloud disk; original preserved at $backup"
