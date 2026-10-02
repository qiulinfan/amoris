// Bun's JavaScriptCore drops the API lock from these C API functions (Bun holds it for the life of
// its process), where upstream WebKit takes it in each; without it a debug build asserts and a
// release build corrupts the heap it tears down. These wrappers take the lock again, and
// pocket_jsc.def exports them under the C API's own names.
#include <cmakeconfig.h>
#include <JavaScriptCore/APICast.h>
#include <JavaScriptCore/JSGlobalObject.h>
#include <JavaScriptCore/JSLock.h>
#include <JavaScriptCore/JavaScript.h>
#include <JavaScriptCore/VM.h>

using JSC::JSLockHolder;

#define LOCKED(ctx) JSLockHolder locker(toJS(ctx)->vm())

extern "C" {

bool pocket_JSCheckScriptSyntax(JSContextRef ctx, JSStringRef script, JSStringRef url, int line, JSValueRef* exception) {
    LOCKED(ctx);
    return JSCheckScriptSyntax(ctx, script, url, line, exception);
}

void pocket_JSGarbageCollect(JSContextRef ctx) {
    if (!ctx) return;
    LOCKED(ctx);
    JSGarbageCollect(ctx);
}

JSGlobalContextRef pocket_JSGlobalContextRetain(JSGlobalContextRef ctx) {
    LOCKED(ctx);
    return JSGlobalContextRetain(ctx);
}

// The holder keeps the VM alive until it unlocks, as upstream's does.
void pocket_JSGlobalContextRelease(JSGlobalContextRef ctx) {
    LOCKED(ctx);
    JSGlobalContextRelease(ctx);
}

JSObjectRef pocket_JSContextGetGlobalObject(JSContextRef ctx) {
    LOCKED(ctx);
    return JSContextGetGlobalObject(ctx);
}

JSGlobalContextRef pocket_JSContextGetGlobalContext(JSContextRef ctx) {
    LOCKED(ctx);
    return JSContextGetGlobalContext(ctx);
}

JSStringRef pocket_JSGlobalContextCopyName(JSGlobalContextRef ctx) {
    LOCKED(ctx);
    return JSGlobalContextCopyName(ctx);
}

void pocket_JSGlobalContextSetName(JSGlobalContextRef ctx, JSStringRef name) {
    LOCKED(ctx);
    JSGlobalContextSetName(ctx, name);
}

JSTypedArrayType pocket_JSValueGetTypedArrayType(JSContextRef ctx, JSValueRef value, JSValueRef* exception) {
    LOCKED(ctx);
    return JSValueGetTypedArrayType(ctx, value, exception);
}

JSObjectRef pocket_JSObjectMakeTypedArray(JSContextRef ctx, JSTypedArrayType type, size_t length, JSValueRef* exception) {
    LOCKED(ctx);
    return JSObjectMakeTypedArray(ctx, type, length, exception);
}

JSObjectRef pocket_JSObjectMakeTypedArrayWithBytesNoCopy(JSContextRef ctx, JSTypedArrayType type, void* bytes, size_t length, JSTypedArrayBytesDeallocator dealloc, void* context, JSValueRef* exception) {
    LOCKED(ctx);
    return JSObjectMakeTypedArrayWithBytesNoCopy(ctx, type, bytes, length, dealloc, context, exception);
}

JSObjectRef pocket_JSObjectMakeTypedArrayWithArrayBuffer(JSContextRef ctx, JSTypedArrayType type, JSObjectRef buffer, JSValueRef* exception) {
    LOCKED(ctx);
    return JSObjectMakeTypedArrayWithArrayBuffer(ctx, type, buffer, exception);
}

JSObjectRef pocket_JSObjectMakeTypedArrayWithArrayBufferAndOffset(JSContextRef ctx, JSTypedArrayType type, JSObjectRef buffer, size_t offset, size_t length, JSValueRef* exception) {
    LOCKED(ctx);
    return JSObjectMakeTypedArrayWithArrayBufferAndOffset(ctx, type, buffer, offset, length, exception);
}

void* pocket_JSObjectGetTypedArrayBytesPtr(JSContextRef ctx, JSObjectRef object, JSValueRef* exception) {
    LOCKED(ctx);
    return JSObjectGetTypedArrayBytesPtr(ctx, object, exception);
}

JSObjectRef pocket_JSObjectGetTypedArrayBuffer(JSContextRef ctx, JSObjectRef object, JSValueRef* exception) {
    LOCKED(ctx);
    return JSObjectGetTypedArrayBuffer(ctx, object, exception);
}

JSObjectRef pocket_JSObjectMakeArrayBufferWithBytesNoCopy(JSContextRef ctx, void* bytes, size_t length, JSTypedArrayBytesDeallocator dealloc, void* context, JSValueRef* exception) {
    LOCKED(ctx);
    return JSObjectMakeArrayBufferWithBytesNoCopy(ctx, bytes, length, dealloc, context, exception);
}

void* pocket_JSObjectGetArrayBufferBytesPtr(JSContextRef ctx, JSObjectRef object, JSValueRef* exception) {
    LOCKED(ctx);
    return JSObjectGetArrayBufferBytesPtr(ctx, object, exception);
}

}  // extern "C"
