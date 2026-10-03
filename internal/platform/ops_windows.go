package platform

import (
	"errors"
	"strconv"
	"syscall"
	"unsafe"
)

// TrashName is the user-facing name of the trash.
const TrashName = "Recycle Bin"

// Reveal selects the item in Explorer.
func Reveal(path string) error {
	if err := checkAbs(path); err != nil {
		return err
	}
	return startDetached("explorer.exe", "/select,", path)
}

var (
	shell32          = syscall.NewLazyDLL("shell32.dll")
	procSHFileOpertn = shell32.NewProc("SHFileOperationW")
)

// shFileOpStruct mirrors SHFILEOPSTRUCTW (64-bit natural alignment).
type shFileOpStruct struct {
	hwnd                  uintptr
	wFunc                 uint32
	pFrom                 *uint16
	pTo                   *uint16
	fFlags                uint16
	fAnyOperationsAborted int32
	hNameMappings         uintptr
	lpszProgressTitle     *uint16
}

const (
	foDelete          = 0x3
	fofSilent         = 0x4
	fofNoConfirmation = 0x10
	fofAllowUndo      = 0x40
	fofNoErrorUI      = 0x400
	fofNoConfirmMkdir = 0x200
)

// Trash moves path to the Recycle Bin via SHFileOperationW with
// FOF_ALLOWUNDO. It never deletes permanently. want is not checked here:
// Windows exposes no owner or inode identity through the scan.
func Trash(path string, _ Expect) error {
	if err := checkAbs(path); err != nil {
		return err
	}
	if unsafe.Sizeof(uintptr(0)) != 8 {
		return errors.New("recycle bin support requires a 64-bit build")
	}
	u, err := syscall.UTF16FromString(path)
	if err != nil {
		return err
	}
	u = append(u, 0) // double NUL terminated list
	op := shFileOpStruct{wFunc: foDelete, pFrom: &u[0],
		fFlags: fofAllowUndo | fofNoConfirmation | fofSilent | fofNoErrorUI | fofNoConfirmMkdir}
	r, _, _ := procSHFileOpertn.Call(uintptr(unsafe.Pointer(&op)))
	if r != 0 {
		return errors.New("SHFileOperation failed: code " + strconv.Itoa(int(r)))
	}
	if op.fAnyOperationsAborted != 0 {
		return errors.New("operation aborted")
	}
	return nil
}

// Headless is always false: Windows always has Explorer and the Recycle Bin.
func Headless() bool { return false }

// Delete is not offered on Windows; items go to the Recycle Bin.
func Delete(string, Expect) error { return errors.New("permanent delete not supported on Windows") }
