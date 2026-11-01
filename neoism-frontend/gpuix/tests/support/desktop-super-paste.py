# Opt-in Linux desktop test: emit Super+V through a temporary input device.
# Unlike GPUIX key injection, this exercises the compositor shortcut binding.
import os,fcntl,struct,time
fd=os.open('/dev/uinput',os.O_WRONLY|os.O_NONBLOCK)
try:
 fcntl.ioctl(fd,0x40045564,1)
 for key in [47,125]:fcntl.ioctl(fd,0x40045565,key)
 fcntl.ioctl(fd,0x405c5503,struct.pack('HHHH80sI',3,0x1234,0x5678,1,b'Neoism temporary shortcut test',0))
 fcntl.ioctl(fd,0x5501);time.sleep(.8)
 def key(code,value):
  os.write(fd,struct.pack('llHHi',0,0,1,code,value));os.write(fd,struct.pack('llHHi',0,0,0,0,0))
 key(125,1);time.sleep(.05);key(47,1);time.sleep(.1);key(47,0);key(125,0);time.sleep(.2)
finally:
 fcntl.ioctl(fd,0x5502);os.close(fd)
