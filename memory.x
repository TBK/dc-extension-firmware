/* Flash layout (2 MiB part), shared with bootloader/memory.x:
 *
 *   0x10000000  BOOT2             (0x100)   installed with the bootloader
 *   0x10000100  bootloader code   (to 0x6000), never rewritten by updates
 *   0x10006000  BOOTLOADER_STATE  (4K)      embassy-boot swap state
 *   0x10007000  ACTIVE            (256K)    this application
 *   0x10080000  settings sector   (4K)      power state / restore mode
 *   0x10100000  DFU               (260K)    firmware updates staged here
 *
 * The application is linked into ACTIVE; boot2 comes from the bootloader
 * image (embassy-rp feature `boot2-none`).
 */
MEMORY
{
    BOOT2            : ORIGIN = 0x10000000, LENGTH = 0x100
    BOOTLOADER_STATE : ORIGIN = 0x10006000, LENGTH = 4K
    FLASH            : ORIGIN = 0x10007000, LENGTH = 256K
    DFU              : ORIGIN = 0x10100000, LENGTH = 260K
    RAM              : ORIGIN = 0x20000000, LENGTH = 256K
}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(BOOT2);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(BOOT2);

__bootloader_dfu_start = ORIGIN(DFU) - ORIGIN(BOOT2);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(BOOT2);
