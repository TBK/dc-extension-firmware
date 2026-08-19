/* Flash layout (2 MiB part), shared with the application's memory.x:
 *
 *   0x10000000  BOOT2             (0x100)
 *   0x10000100  bootloader code   (to 0x6000)
 *   0x10006000  BOOTLOADER_STATE  (4K)
 *   0x10007000  ACTIVE            (256K)  <- application runs here
 *   0x10080000  settings sector   (4K, owned by the application)
 *   0x10100000  DFU               (260K)  <- updates staged here
 */
MEMORY
{
    BOOT2            : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH            : ORIGIN = 0x10000100, LENGTH = 24K - 0x100
    BOOTLOADER_STATE : ORIGIN = 0x10006000, LENGTH = 4K
    ACTIVE           : ORIGIN = 0x10007000, LENGTH = 256K
    DFU              : ORIGIN = 0x10100000, LENGTH = 260K
    RAM              : ORIGIN = 0x20000000, LENGTH = 256K
}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(BOOT2);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(BOOT2);

__bootloader_active_start = ORIGIN(ACTIVE) - ORIGIN(BOOT2);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE) - ORIGIN(BOOT2);

__bootloader_dfu_start = ORIGIN(DFU) - ORIGIN(BOOT2);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(BOOT2);

EXTERN(BOOT2_FIRMWARE)

SECTIONS {
    .boot2 ORIGIN(BOOT2) :
    {
        KEEP(*(.boot2));
    } > BOOT2
} INSERT BEFORE .text;
