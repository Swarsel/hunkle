;;; hunkle-test.el --- Tests for hunkle.el -*- lexical-binding: t; -*-

;;; Commentary:

;; ERT tests that drive hunkle.el against real scratch repositories and
;; the real hunkle binary.  After `cargo build', run (from anywhere):
;;
;;   emacs --batch -l tests/hunkle-test.el -f ert-run-tests-batch-and-exit

;;; Code:

(require 'ert)

;; Locate the repo root from this file's path (tests/ -> repo root), so the
;; test works regardless of the invocation directory.
(defvar hunkle-test--root
  (file-name-directory
   (directory-file-name
    (file-name-directory (or load-file-name buffer-file-name)))))

(add-to-list 'load-path (expand-file-name "emacs" hunkle-test--root))
(require 'hunkle)

(setq hunkle-executable
      (or (getenv "HUNKLE_BIN")
          (expand-file-name "target/debug/hunkle" hunkle-test--root)))

(defun hunkle-test--git (dir &rest args)
  "Run git ARGS in DIR; return stdout, failing the test on error."
  (with-temp-buffer
    (let ((status (apply #'call-process "git" nil t nil "-C" dir args)))
      (unless (eql status 0)
        (error "git %s failed: %s" (string-join args " ") (buffer-string)))
      (buffer-string))))

(defun hunkle-test--make-repo ()
  "Create a scratch repo with staged changes; return its directory."
  (let ((dir (make-temp-file "hunkle-test" t)))
    (hunkle-test--git dir "init" "-b" "main")
    (hunkle-test--git dir "config" "user.name" "test")
    (hunkle-test--git dir "config" "user.email" "test@example.com")
    (hunkle-test--git dir "config" "commit.gpgsign" "false")
    (with-temp-file (expand-file-name "f.txt" dir)
      (dotimes (i 20) (insert (format "line%d\n" (1+ i)))))
    (hunkle-test--git dir "add" "-A")
    (hunkle-test--git dir "commit" "-m" "base")
    ;; Two hunks in f.txt plus a new file.
    (with-temp-file (expand-file-name "f.txt" dir)
      (dotimes (i 20)
        (let ((n (1+ i)))
          (insert (if (memq n '(2 18))
                      (format "LINE%d\n" n)
                    (format "line%d\n" n))))))
    (with-temp-file (expand-file-name "new.txt" dir)
      (insert "alpha\nbeta\n"))
    (hunkle-test--git dir "add" "-A")
    dir))

(defmacro hunkle-test--with-buffer (dir &rest body)
  "Open the hunkle buffer for the repo in DIR and run BODY inside it."
  (declare (indent 1))
  `(let ((default-directory (file-name-as-directory ,dir)))
     (hunkle)
     (unwind-protect
         (progn ,@body)
       (kill-buffer))))

(ert-deftest hunkle-loads-and-renders ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (should (eq major-mode 'hunkle-mode))
      (should (= (length hunkle--files) 2))
      (should (equal hunkle--branch "main"))
      (should (string-match-p "Staged changes (6 unassigned lines)"
                              (buffer-string)))
      (should (string-match-p "@@ -15,6 \\+15,6 @@" (buffer-string)))
      (should (string-match-p "\\+LINE2" (buffer-string))))))

(ert-deftest hunkle-assign-and-create-commits ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list "first" "second"))
      (hunkle--assign-locs (hunkle--hunk-locs 0 0) 0)
      (hunkle--assign-locs (hunkle--hunk-locs 0 1) 1)
      ;; Only "alpha" of new.txt goes into the first commit.
      (hunkle--assign-locs (list (car (hunkle--hunk-locs 1 0))) 0)
      (should (= (hunkle--unassigned-count) 1))
      (should (equal (hunkle--commit-stats 0) '(2 . 1)))
      ;; Assigned lines are tagged in the buffer.
      (should (string-match-p "\\[1\\] +\\+LINE2" (buffer-string)))
      (let ((commits (hunkle--apply)))
        (should (= (length commits) 2))
        (should (equal (alist-get 'message (car commits)) "first"))))
    (should (equal (split-string (hunkle-test--git dir "log" "--format=%s"))
                   '("second" "first" "base")))
    (should (equal (hunkle-test--git dir "show" "HEAD:new.txt") "alpha\n"))
    ;; "beta" remains staged; the working tree is untouched.
    (should (string-match-p "\\+beta"
                            (hunkle-test--git dir "diff" "--cached")))
    (should (equal (hunkle-test--git dir "diff") ""))))

(ert-deftest hunkle-begin-selection-assigns-only-selected-lines ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list "c"))
      ;; v on the +alpha line, no movement: selects just that line.
      (goto-char (point-min))
      (search-forward "+alpha")
      (forward-line 0)
      (let ((loc (get-text-property (point) 'hunkle-loc)))
        (should loc)
        (hunkle-begin-selection)
        (should mark-active)
        (hunkle-assign-number 1)
        ;; Exactly that one line went to commit 0; "beta" stays pending.
        (should (eql (gethash loc hunkle--assign) 0))
        (should (= (hunkle--unassigned-count) 5))))))

(ert-deftest hunkle-select-line-grabs-whole-line-from-mid-column ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list "c"))
      (goto-char (point-min))
      (search-forward "+alpha")
      (forward-line 0)
      (let ((loc (get-text-property (point) 'hunkle-loc)))
        (should loc)
        ;; Point sitting mid-line must not matter -- V grabs the line.
        (forward-char 4)
        (hunkle-select-line)
        (should mark-active)
        (hunkle-assign-number 1)
        (should (eql (gethash loc hunkle--assign) 0))
        (should (= (hunkle--unassigned-count) 5))))))

(ert-deftest hunkle-hunk-at-point-targets ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (goto-char (point-min))
      (search-forward "+LINE18")
      (let ((locs (hunkle--targets)))
        ;; The whole second hunk of f.txt: one del + one add.
        (should (= (length locs) 2))
        (should (equal (mapcar #'cadr locs) '(1 1)))))))

(ert-deftest hunkle-swap-remaps-assignments ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list "a" "b"))
      (hunkle--assign-locs (hunkle--hunk-locs 0 0) 0)
      (hunkle--assign-locs (hunkle--hunk-locs 0 1) 1)
      (cl-letf (((symbol-function 'read-number) (lambda (&rest _) 2))
                ((symbol-function 'hunkle--commit-at-point-or-read)
                 (lambda (&rest _) 0)))
        (hunkle-swap-commits))
      (should (equal hunkle--commits '("b" "a")))
      (should (eql (gethash (car (hunkle--hunk-locs 0 0)) hunkle--assign) 1))
      (should (eql (gethash (car (hunkle--hunk-locs 0 1)) hunkle--assign) 0)))))

(ert-deftest hunkle-stale-token-rejected ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list "x"))
      (hunkle--assign-locs (hunkle--hunk-locs 0 0) 0)
      ;; Staged state changes behind our back.
      (with-temp-file (expand-file-name "other.txt" dir)
        (insert "surprise\n"))
      (hunkle-test--git dir "add" "-A")
      (should-error (hunkle--apply)))))

(provide 'hunkle-test)
;;; hunkle-test.el ends here
