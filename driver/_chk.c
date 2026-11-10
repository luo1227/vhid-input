#include "vhidinput.h"
int _chk_size = sizeof(VHF_CONFIG);
int _chk_km =
#ifdef _KERNEL_MODE
1;
#else
0;
#endif
