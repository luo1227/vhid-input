#pragma once

// The KMDF headers live outside the normal kernel include path and are not
// pulled in by the kit props on this machine, so this TU includes them in
// the documented order: kernel types first, then driverspecs, then wdf.h.

#include <ntddk.h>

#ifndef _Analysis_mode_
#define _Analysis_mode_(x)
#endif
#ifndef KMDF_INCLUDED
#define KMDF_INCLUDED 1
#endif

#if _MSC_VER >= 1000
#pragma once
#endif
#include <driverspecs.h>

#include <wdf.h>
#include <vhf.h>
