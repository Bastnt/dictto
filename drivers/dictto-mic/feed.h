#pragma once
#include <ntddk.h>

void DicttoInstallFeedDispatch(PDRIVER_OBJECT driver);
NTSTATUS DicttoCreateFeed(PDEVICE_OBJECT adapter);
void DicttoRemoveFeed(PDEVICE_OBJECT adapter);
void DicttoSetRunning(PDEVICE_OBJECT adapter, const void* stream, BOOLEAN running);
void DicttoReadPcm(const void* stream, PUCHAR destination, ULONG length);
