;;; hunkle-test.el --- Tests for hunkle.el -*- lexical-binding: t; -*-

;;; Commentary:

;; ERT tests that drive hunkle.el against real scratch repositories and
;; the real hunkle binary.  After `cargo build', run (from anywhere):
;;
;;   emacs --batch -l tests/hunkle-test.el -f ert-run-tests-batch-and-exit

;;; Code:

(require 'ert)

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
      (setq hunkle--commits (list (cons "first" nil) (cons "second" nil)))
      (hunkle--assign-locs (hunkle--hunk-locs 0 0) 0)
      (hunkle--assign-locs (hunkle--hunk-locs 0 1) 1)
      (hunkle--assign-locs (list (car (hunkle--hunk-locs 1 0))) 0)
      (should (= (hunkle--unassigned-count) 1))
      (should (equal (hunkle--commit-stats 0) '(2 . 1)))
      (should (string-match-p "\\[1\\] +\\+LINE2" (buffer-string)))
      (cl-letf (((symbol-function 'y-or-n-p) (lambda (&rest _) t)))
        (hunkle-create-commits)))
    (should (equal (split-string (hunkle-test--git dir "log" "--format=%s"))
                   '("second" "first" "base")))
    (should (equal (hunkle-test--git dir "show" "HEAD:new.txt") "alpha\n"))
    (should (string-match-p "\\+beta"
                            (hunkle-test--git dir "diff" "--cached")))
    (should (equal (hunkle-test--git dir "diff") ""))))

(ert-deftest hunkle-begin-selection-assigns-only-selected-lines ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list (cons "c" nil)))
      (goto-char (point-min))
      (search-forward "+alpha")
      (forward-line 0)
      (let ((loc (get-text-property (point) 'hunkle-loc)))
        (should loc)
        (hunkle-begin-selection)
        (should mark-active)
        (hunkle-assign-number 1)
        (should (eql (gethash loc hunkle--assign) 0))
        (should (= (hunkle--unassigned-count) 5))))))

(ert-deftest hunkle-select-line-grabs-whole-line-from-mid-column ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list (cons "c" nil)))
      (goto-char (point-min))
      (search-forward "+alpha")
      (forward-line 0)
      (let ((loc (get-text-property (point) 'hunkle-loc)))
        (should loc)
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
        (should (= (length locs) 2))
        (should (equal (mapcar #'cadr locs) '(1 1)))))))

(ert-deftest hunkle-swap-remaps-assignments ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list (cons "a" nil) (cons "b" nil)))
      (hunkle--assign-locs (hunkle--hunk-locs 0 0) 0)
      (hunkle--assign-locs (hunkle--hunk-locs 0 1) 1)
      (cl-letf (((symbol-function 'read-number) (lambda (&rest _) 2))
                ((symbol-function 'hunkle--commit-at-point-or-read)
                 (lambda (&rest _) 0)))
        (hunkle-swap-commits))
      (should (equal hunkle--commits '(("b") ("a"))))
      (should (eql (gethash (car (hunkle--hunk-locs 0 0)) hunkle--assign) 1))
      (should (eql (gethash (car (hunkle--hunk-locs 0 1)) hunkle--assign) 0)))))

(ert-deftest hunkle-branch-commit-creates-branch ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list (cons "local" nil) (cons "side" "topic")))
      (hunkle--assign-locs (hunkle--hunk-locs 0 0) 0)
      (hunkle--assign-locs (hunkle--hunk-locs 0 1) 1)
      (hunkle--assign-locs (hunkle--hunk-locs 1 0) 1)
      (should (string-match-p "side -> topic" (buffer-string)))
      (let* ((result (hunkle--apply))
             (commits (alist-get 'commits result)))
        (should (= (length commits) 2))
        (should (equal (alist-get 'branch (cadr commits)) "topic"))
        (should (null (alist-get 'worktree_skipped result)))))
    (should (equal (split-string (hunkle-test--git dir "log" "--format=%s"))
                   '("local" "base")))
    (should (equal (split-string
                    (hunkle-test--git dir "log" "--format=%s" "topic"))
                   '("side" "base")))
    (should (equal (hunkle-test--git dir "show" "topic:new.txt") "alpha\nbeta\n"))
    (should-not (file-exists-p (expand-file-name "new.txt" dir)))
    (should (equal (hunkle-test--git dir "diff" "--cached") ""))
    (should (equal (hunkle-test--git dir "diff") ""))))

(ert-deftest hunkle-fully-assigned-hunk-moves-to-assigned-section ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list (cons "c" nil)))
      (hunkle--assign-locs (hunkle--hunk-locs 0 0) 0)
      (let* ((s (buffer-string))
             (sp (string-match "Staged changes (4 unassigned lines)" s))
             (ap (string-match "Assigned changes (2 assigned lines)" s)))
        (should sp)
        (should ap)
        (should (< sp ap))
        (let ((staged (substring s sp ap))
              (assigned (substring s ap)))
          (should-not (string-match-p "\\+LINE2" staged))
          (should (string-match-p "\\+LINE2" assigned))
          (should (string-match-p "\\+LINE18" staged)))))))

(defun hunkle-test--assign-digit-1 ()
  "Assign the target to commit 1 just as the `1' key would."
  (let ((last-command-event ?1))
    (hunkle-assign-digit)))

(ert-deftest hunkle-assign-on-file-heading-assigns-whole-file ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list (cons "c" nil)))
      (goto-char (point-min))
      (search-forward "f.txt")
      (forward-line 0)
      (should-not (hunkle--hunk-at-point))
      (should (hunkle--file-at-point))
      (hunkle-test--assign-digit-1)
      (should (hunkle--hunk-fully-assigned-p 0 0))
      (should (hunkle--hunk-fully-assigned-p 0 1))
      (should (eql (gethash (car (hunkle--hunk-locs 0 0)) hunkle--assign) 0))
      (should (eql (gethash (car (hunkle--hunk-locs 0 1)) hunkle--assign) 0))
      (should-not (gethash (car (hunkle--hunk-locs 1 0)) hunkle--assign)))))

(ert-deftest hunkle-cursor-stays-in-staged-after-assign ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list (cons "c" nil)))
      (goto-char (point-min))
      (search-forward "+LINE2")
      (forward-line 0)
      (should (equal (cl-subseq (get-text-property (point) 'hunkle-loc) 0 2) '(0 0)))
      (hunkle--assign-locs (hunkle--hunk-locs 0 0) 0)
      (let ((loc (get-text-property (pos-bol) 'hunkle-loc)))
        (should loc)
        (should (eq (hunkle--loc-group loc) 'staged))))))

(ert-deftest hunkle-cursor-stays-in-assigned-after-unassign ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list (cons "c" nil)))
      (hunkle--assign-locs (hunkle--hunk-locs 0 0) 0)
      (hunkle--assign-locs (hunkle--hunk-locs 0 1) 0)
      (when magit-root-section (magit-section-show magit-root-section))
      (goto-char (point-min))
      (search-forward "Assigned changes")
      (search-forward "+LINE2")
      (forward-line 0)
      (should (eq (hunkle--loc-group (get-text-property (point) 'hunkle-loc)) 'assigned))
      (hunkle-unassign)
      (let ((loc (get-text-property (pos-bol) 'hunkle-loc)))
        (should loc)
        (should (eq (hunkle--loc-group loc) 'assigned))))))

(ert-deftest hunkle-folded-section-stays-consistent-across-render ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (cl-flet ((file-sec ()
                  (goto-char (point-min))
                  (search-forward "f.txt")
                  (forward-line 0)
                  (magit-current-section))
                (body-invisible-p (sec)
                  (seq-some (lambda (o) (overlay-get o 'invisible))
                            (overlays-at (1+ (oref sec content))))))
        (magit-section-hide (file-sec))
        (hunkle--render)
        (let ((sec (file-sec)))
          (should (oref sec hidden))
          (should (body-invisible-p sec))
          (magit-section-toggle sec)
          (should-not (oref sec hidden))
          (should-not (body-invisible-p sec)))))))

(ert-deftest hunkle-create-commits-recovers-from-stale-token ()
  (let ((dir (hunkle-test--make-repo)))
    (hunkle-test--with-buffer dir
      (setq hunkle--commits (list (cons "x" nil)))
      (hunkle--assign-locs (hunkle--hunk-locs 0 0) 0)
      (with-temp-file (expand-file-name "other.txt" dir)
        (insert "surprise\n"))
      (hunkle-test--git dir "add" "-A")
      (cl-letf (((symbol-function 'y-or-n-p) (lambda (&rest _) t))
                ((symbol-function 'yes-or-no-p) (lambda (&rest _) t)))
        (hunkle-create-commits))
      (should (hash-table-empty-p hunkle--assign))
      (should (null hunkle--commits))
      (should (string-match-p "other.txt" (buffer-string))))))

(provide 'hunkle-test)
;;; hunkle-test.el ends here
