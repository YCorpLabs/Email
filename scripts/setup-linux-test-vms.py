#!/usr/bin/env python3
"""Prepare an isolated UTM Ubuntu test VM from a verified official cloud image.
Schema: https://github.com/utmapp/UTM/tree/v5.0.8/Configuration
No mail accounts, host directories, or host credentials are copied into the VM.
"""
import argparse
import json
import os
from pathlib import Path
import plistlib
import secrets
import subprocess
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--root', type=Path, default=Path.home() / 'Virtual Machines/Email')
args = parser.parse_args()
root = args.root.expanduser().resolve()
vm = root / 'Email Ubuntu 24.04.utm'
if vm.exists():
    raise SystemExit('VM already exists; refusing to overwrite its disk or identity.')
data = vm / 'Data'
data.mkdir(parents=True)
os.chmod(root, 0o700)
access = root / 'Access'
access.mkdir(exist_ok=True, mode=0o700)
key = access / 'ubuntu_ed25519'
subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', 'email-ubuntu-test', '-f', str(key)], check=True)
password = secrets.token_urlsafe(20)
password_hash = subprocess.run(['/opt/homebrew/opt/openssl@3/bin/openssl', 'passwd', '-6', '-stdin'], input=password+'\n', text=True, capture_output=True, check=True).stdout.strip()
credentials = access / 'ubuntu-login.txt'
credentials.write_text('VM user: emailtest\nVM password: '+password+'\nSSH: ssh -i "'+str(key)+'" -p 22422 emailtest@127.0.0.1\n')
credentials.chmod(0o600)
seed = access / 'ubuntu-seed'
seed.mkdir(mode=0o700)
user_data = '''#cloud-config
hostname: email-ubuntu-test
manage_etc_hosts: true
users:
  - name: emailtest
    gecos: Email Linux Test
    groups: [adm, sudo]
    shell: /bin/bash
    lock_passwd: false
    passwd: HASH
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys:
      - PUBKEY
ssh_pwauth: false
package_update: true
packages: [ubuntu-desktop-minimal, qemu-guest-agent, spice-vdagent, gnome-keyring, openssh-server]
runcmd:
  - [systemctl, enable, --now, qemu-guest-agent]
  - [netplan, apply]
  - [systemctl, set-default, graphical.target]
  - [touch, /home/emailtest/.email-vm-provisioned]
  - [systemctl, start, gdm3]
'''.replace('HASH', password_hash).replace('PUBKEY', key.with_suffix('.pub').read_text().strip())
(seed / 'user-data').write_text(user_data)
(seed / 'user-data').chmod(0o600)
(seed / 'meta-data').write_text('instance-id: email-ubuntu-'+str(uuid.uuid4())+'\nlocal-hostname: email-ubuntu-test\n')
subprocess.run(['hdiutil', 'makehybrid', '-o', str(data / 'seed.iso'), '-iso', '-joliet', '-default-volume-name', 'cidata', str(seed)], check=True, stdout=subprocess.DEVNULL)
subprocess.run(['cp', '-c', str(root / 'Installers/ubuntu-noble-arm64.img'), str(data / 'disk.qcow2')], check=True)
# USB seed CDs can appear after cloud-init datasource detection in UTM.
# An on-disk NoCloud seed is detected deterministically before networking.
subprocess.run([str(Path(__file__).with_name('seed-ubuntu-test-disk.sh')), str(data / 'disk.qcow2'), str(seed)], check=True)
config = {
 'Backend': 'QEMU', 'ConfigurationVersion': 4,
 'Information': {'Name':'Email Ubuntu 24.04', 'UUID':str(uuid.uuid4()), 'IconCustom':False, 'Notes':'Isolated ARM64 Ubuntu desktop for Email QA. SSH is forwarded only on host loopback port 22422. No host directory or clipboard sharing.'},
 'System': {'Architecture':'aarch64','Target':'virt','CPU':'default','CPUFlagsAdd':[],'CPUFlagsRemove':[],'CPUCount':4,'ForceMulticore':False,'MemorySize':6144,'JITCacheSize':0},
 'QEMU': {'MachinePropertyOverride':'gic-version=3,acpi=on','DebugLog':True,'UEFIBoot':True,'RNGDevice':True,'BalloonDevice':True,'TPMDevice':False,'Hypervisor':True,'RTCLocalTime':False,'PS2Controller':False,'AdditionalArguments':[]},
 'Input': {'UsbBusSupport':'3.0','UsbSharing':False,'MaximumUsbShare':0},
 'Sharing': {'DirectoryShareMode':'None','DirectoryShareReadOnly':True,'ClipboardSharing':False},
 'Display': [{'Hardware':'virtio-ramfb','DynamicResolution':True,'UpscalingFilter':'Linear','DownscalingFilter':'Linear','NativeResolution':False}],
 'Drive': [
  {'ImageName':'disk.qcow2','ImageType':'Disk','Interface':'VirtIO','InterfaceVersion':1,'Identifier':str(uuid.uuid4()),'ReadOnly':False},
  {'ImageName':'seed.iso','ImageType':'CD','Interface':'USB','InterfaceVersion':1,'Identifier':str(uuid.uuid4()),'ReadOnly':True}],
 'Network': [{'Mode':'Emulated','Hardware':'virtio-net-pci','MacAddress':'52:54:00:ea:11:01','IsolateFromHost':False,'PortForward':[{'Protocol':'TCP','HostAddress':'127.0.0.1','HostPort':22422,'GuestPort':22}]}],
 'Serial': [{'Mode':'Terminal','Target':'Auto','Terminal':{'Font':'Menlo','FontSize':12,'ForegroundColor':'#ffffff','BackgroundColor':'#000000','CursorBlink':True}}], 'Sound': []
}
with (vm / 'config.plist').open('wb') as f: plistlib.dump(config, f)
print('Prepared '+str(vm))
print('Private VM login details: '+str(credentials))
print('Resize Data/disk.qcow2 to 40G before first boot.')
