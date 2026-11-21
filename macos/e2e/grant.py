#!/usr/bin/env python3
"""测试辅助（只在 CI 的 macOS 虚拟机上用）：在 TCC 数据库里给测试程序授予辅助功能、模拟输入和录屏权限。"""
import sqlite3, sys, time

db_path, clients = sys.argv[1], sys.argv[2:]
db = sqlite3.connect(db_path)
columns = [row[1] for row in db.execute("PRAGMA table_info(access)")]
values = {
    "client_type": 1, "auth_value": 2, "auth_reason": 4, "auth_version": 1,
    "indirect_object_identifier_type": 0, "indirect_object_identifier": "UNUSED",
    "flags": 0, "last_modified": int(time.time()), "boot_uuid": "UNUSED", "last_reminded": 0,
    "allowed": 1, "prompt_count": 1,
}
for client in clients:
    for service in ("kTCCServiceAccessibility", "kTCCServicePostEvent", "kTCCServiceScreenCapture",
                    "kTCCServiceAppleEvents", "kTCCServiceListenEvent"):
        row = dict(values, service=service, client=client)
        if client.startswith("/"):
            row["client_type"] = 1
        else:
            row["client_type"] = 0
        names = [c for c in columns if c in row]
        try:
            db.execute(f"INSERT OR REPLACE INTO access ({','.join(names)}) VALUES ({','.join('?' * len(names))})",
                       [row[c] for c in names])
        except sqlite3.Error as error:
            print(f"{db_path} {service} {client}: {error}")
db.commit()
print(f"{db_path}: 已授权 {len(clients)} 个程序（列：{len(columns)}）")
