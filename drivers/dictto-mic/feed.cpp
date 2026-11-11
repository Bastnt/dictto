#include "feed.h"
#include <wdmsec.h>

namespace {
constexpr DEVICE_TYPE FeedDeviceType = 0x8000;
const GUID FeedClass = { 0xea3eed73, 0xde68, 0x457b, { 0xbe, 0xca, 0x81, 0x78, 0xe3, 0xff, 0xaf, 0x2a } };
const ULONG WritePcm = CTL_CODE(FeedDeviceType, 0x801, METHOD_BUFFERED, FILE_WRITE_ACCESS);
const ULONG Flush = CTL_CODE(FeedDeviceType, 0x802, METHOD_BUFFERED, FILE_WRITE_ACCESS);
const ULONG QueryDepth = CTL_CODE(FeedDeviceType, 0x803, METHOD_BUFFERED, FILE_WRITE_ACCESS);
const ULONG QueryActive = CTL_CODE(FeedDeviceType, 0x804, METHOD_BUFFERED, FILE_WRITE_ACCESS);

UNICODE_STRING linkName = RTL_CONSTANT_STRING(L"\\DosDevices\\Global\\DicttoMicFeed");
FAST_MUTEX lifecycle;
KSPIN_LOCK lock;
PDEVICE_OBJECT owner;
PDEVICE_OBJECT control;
const void* runningStream;
bool lostTail;
UCHAR pcm[19200];
ULONG head, count;
PDRIVER_DISPATCH original[IRP_MJ_MAXIMUM_FUNCTION + 1];

NTSTATUS Dispatch(PDEVICE_OBJECT device, PIRP irp)
{
    auto stack = IoGetCurrentIrpStackLocation(irp);
    if (device->DeviceType != FeedDeviceType)
        return original[stack->MajorFunction](device, irp);

    NTSTATUS status = STATUS_INVALID_DEVICE_REQUEST;
    ULONG transferred = 0;
    KIRQL irql;
    KeAcquireSpinLock(&lock, &irql);
    switch (stack->MajorFunction) {
    case IRP_MJ_CREATE:
        status = device != control ? STATUS_DEVICE_REMOVED :
            (stack->FileObject->FileName.Length ? STATUS_OBJECT_NAME_NOT_FOUND : STATUS_SUCCESS);
        if (NT_SUCCESS(status)) lostTail = false;
        break;
    case IRP_MJ_CLOSE:
    case IRP_MJ_CLEANUP:
        status = STATUS_SUCCESS;
        break;
    case IRP_MJ_DEVICE_CONTROL: {
        auto io = stack->Parameters.DeviceIoControl;
        auto data = static_cast<UCHAR*>(irp->AssociatedIrp.SystemBuffer);
        if (device != control) { status = STATUS_DEVICE_REMOVED; break; }
        if (io.IoControlCode == WritePcm) {
            ULONG length = io.InputBufferLength;
            if (!length || (length & 1) || length > 9600 || io.OutputBufferLength || !data) {
                status = STATUS_INVALID_PARAMETER;
            } else if (!runningStream || lostTail) {
                status = STATUS_DEVICE_NOT_READY;
            } else if (length > sizeof(pcm) - count) {
                status = STATUS_DEVICE_BUSY;
            } else {
                ULONG tail = (head + count) % sizeof(pcm);
                ULONG first = min(length, static_cast<ULONG>(sizeof(pcm)) - tail);
                RtlCopyMemory(pcm + tail, data, first);
                RtlCopyMemory(pcm, data + first, length - first);
                count += length;
                status = STATUS_SUCCESS;
            }
        } else if (io.IoControlCode == Flush) {
            if (io.InputBufferLength || io.OutputBufferLength) { status = STATUS_INVALID_PARAMETER; break; }
            count = head = 0;
            lostTail = false;
            status = STATUS_SUCCESS;
        } else if (io.IoControlCode == QueryDepth || io.IoControlCode == QueryActive) {
            if (io.InputBufferLength || io.OutputBufferLength != sizeof(ULONG) || !data) {
                status = STATUS_INVALID_PARAMETER;
            } else if (io.IoControlCode == QueryDepth && (!runningStream || lostTail)) {
                status = STATUS_DEVICE_NOT_READY;
            } else {
                *reinterpret_cast<ULONG*>(data) = io.IoControlCode == QueryDepth ? count : !!runningStream;
                transferred = sizeof(ULONG);
                status = STATUS_SUCCESS;
            }
        }
        break;
    }
    }
    KeReleaseSpinLock(&lock, irql);
    irp->IoStatus.Status = status;
    irp->IoStatus.Information = transferred;
    IoCompleteRequest(irp, IO_NO_INCREMENT);
    return status;
}
}

void DicttoInstallFeedDispatch(PDRIVER_OBJECT driver)
{
    ExInitializeFastMutex(&lifecycle);
    KeInitializeSpinLock(&lock);
    for (ULONG i = 0; i <= IRP_MJ_MAXIMUM_FUNCTION; ++i) {
        original[i] = driver->MajorFunction[i];
        driver->MajorFunction[i] = Dispatch;
    }
}

NTSTATUS DicttoCreateFeed(PDEVICE_OBJECT adapter)
{
    PAGED_CODE();
    // Serialize device starts/stops across distinct root devnodes.
    KeEnterCriticalRegion();
    ExAcquireFastMutexUnsafe(&lifecycle);
    NTSTATUS status = STATUS_OBJECT_NAME_COLLISION;
    PDEVICE_OBJECT device = nullptr;
    if (!owner) {
        UNICODE_STRING name = RTL_CONSTANT_STRING(L"\\Device\\DicttoMicFeed");
        // Win32 CreateFile adds FILE_READ_ATTRIBUTES to the requested write access.
        UNICODE_STRING acl = RTL_CONSTANT_STRING(L"D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x120196;;;IU)");
        status = IoCreateDeviceSecure(adapter->DriverObject, 0, &name, FeedDeviceType,
                                      FILE_DEVICE_SECURE_OPEN, TRUE, &acl, &FeedClass, &device);
        if (NT_SUCCESS(status)) {
            status = IoCreateSymbolicLink(&linkName, &name);
            if (NT_SUCCESS(status)) {
                KIRQL irql;
                KeAcquireSpinLock(&lock, &irql);
                control = device;
                owner = adapter;
                KeReleaseSpinLock(&lock, irql);
                device->Flags &= ~DO_DEVICE_INITIALIZING;
            } else {
                IoDeleteDevice(device);
            }
        }
    }
    ExReleaseFastMutexUnsafe(&lifecycle);
    KeLeaveCriticalRegion();
    return status;
}

void DicttoRemoveFeed(PDEVICE_OBJECT adapter)
{
    PAGED_CODE();
    KeEnterCriticalRegion();
    ExAcquireFastMutexUnsafe(&lifecycle);
    if (owner == adapter) {
        KIRQL irql;
        KeAcquireSpinLock(&lock, &irql);
        auto device = control;
        control = owner = nullptr;
        runningStream = nullptr;
        head = count = 0;
        KeReleaseSpinLock(&lock, irql);
        IoDeleteSymbolicLink(&linkName);
        IoDeleteDevice(device);
    }
    ExReleaseFastMutexUnsafe(&lifecycle);
    KeLeaveCriticalRegion();
}

void DicttoSetRunning(PDEVICE_OBJECT adapter, const void* stream, BOOLEAN running)
{
    KIRQL irql;
    KeAcquireSpinLock(&lock, &irql);
    if (running && control && adapter == owner) {
        runningStream = stream;
    } else if (!running && runningStream == stream) {
        lostTail = lostTail || count != 0;
        runningStream = nullptr;
        head = count = 0;
    }
    KeReleaseSpinLock(&lock, irql);
}

void DicttoReadPcm(const void* stream, PUCHAR destination, ULONG length)
{
    KIRQL irql;
    KeAcquireSpinLock(&lock, &irql);
    ULONG bytes = runningStream == stream ? min(count, length) : 0;
    if (bytes) {
        ULONG first = min(bytes, static_cast<ULONG>(sizeof(pcm)) - head);
        RtlCopyMemory(destination, pcm + head, first);
        RtlCopyMemory(destination + first, pcm, bytes - first);
        head = (head + bytes) % sizeof(pcm);
        count -= bytes;
    }
    KeReleaseSpinLock(&lock, irql);
    RtlZeroMemory(destination + bytes, length - bytes);
}

