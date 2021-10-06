#!/bin/bash

systemctl stop cabbage-size

cp cabbage-size.service /etc/systemd/system/

systemctl start cabbage-size
systemctl enable cabbage-size

systemctl daemon-reload
