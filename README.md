# NvidiaTeslaExternalFanController

Simple fan controller for a blower fan (or two) for an Nvidia Tesla card. (I'm using an M40, in particular.) The actually fan controller simply sets the fan speed to whatever the temperature reporter running on the computer tells it to. The temperature reporter actually bases the current fan speed the average power usage over the last minute.
