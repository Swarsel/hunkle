;;; hunkle.el --- Split staged changes into commits in a hunk by hunk fashion -*- lexical-binding: t; -*-

;; Author: Leon Schwarzäugl
;; Version: 0.1.0
;; Package-Requires: ((emacs "28.1") (magit "3.3.0"))
;; Keywords: vc, convenience

;;; Commentary:

;; Emacs front end for  hunkle: present all staged hunks in a
;; magit-section buffer and sort them into commits with single keys.
;; `hunkle' opens the buffer; call `hunkle-magit-setup' to bind it to
;; a key in the Magit status buffer (`#' by default).
;;
;; Keys in the hunkle buffer:
;;   n        create a new commit and assign the target to it
;;   1-9      assign the target to that commit
;;   0        assign by commit number
;;   v/V      start a line selection;  move to extend,
;;            then press a commit key to assign just those lines
;;   u        unassign the target
;;   e        edit a commit message
;;   m        swap two commits' numbers (and creation order)
;;   g        reload the staged changes (discards assignments)
;;   C-c C-c  create the commits
;;   C-c C-k  quit without creating commits (changes stay staged)
;;
;; hunkle is released under the MIT license.
;;

;;; Code:

(require 'cl-lib)
(require 'subr-x)
(require 'magit-section)
(require 'magit-diff)

(defgroup hunkle nil
  "Split staged changes into multiple commits."
  :group 'tools)

(defcustom hunkle-executable "hunkle"
  "Name or path of the hunkle binary."
  :type 'string)

(defcustom hunkle-magit-key "#"
  "Key bound to `hunkle' in the Magit status buffer by `hunkle-magit-setup'."
  :type 'string)

(defface hunkle-key
  '((t :inherit magit-hash :weight bold))
  "Face for commit number keys.")

(defface hunkle-tag
  '((t :inherit magit-hash :weight bold))
  "Foreground/weight of the per-line commit tag in the gutter.
The diff line's background still shows through.")

(defun hunkle--face (string face)
  "Propertize STRING with FACE as both `face' and `font-lock-face'."
  (propertize string 'face face 'font-lock-face face))

(defun hunkle--put-face (beg end face)
  "Set FACE on BEG..END as both `face' and `font-lock-face'."
  (put-text-property beg end 'face face)
  (put-text-property beg end 'font-lock-face face))

(defvar-local hunkle--files nil
  "Parsed `files' list from the dump, a list of alists.")
(defvar-local hunkle--token nil)
(defvar-local hunkle--branch nil)
(defvar-local hunkle--commits nil
  "List of commit messages, in creation order.")
(defvar-local hunkle--assign nil
  "Hash table mapping (FILE HUNK LINE) index lists to commit indices.")

(defun hunkle--call (input &rest args)
  "Run the hunkle binary with ARGS and INPUT on stdin; return stdout."
  (let ((stderr-file (make-temp-file "hunkle-stderr")))
    (unwind-protect
        (with-temp-buffer
          (let* ((out (current-buffer))
                 (status
                  (if input
                      (with-temp-buffer
                        (insert input)
                        (apply #'call-process-region (point-min) (point-max)
                               hunkle-executable nil (list out stderr-file)
                               nil args))
                    (apply #'call-process hunkle-executable nil
                           (list out stderr-file) nil args))))
            (unless (eql status 0)
              (error "%s %s failed: %s" hunkle-executable
                     (string-join args " ")
                     (with-temp-buffer
                       (insert-file-contents stderr-file)
                       (string-trim (buffer-string)))))
            (buffer-string)))
      (delete-file stderr-file))))

(defun hunkle--load ()
  "Read the staged hunks into the buffer-local state."
  (let ((dump (json-parse-string (hunkle--call nil "dump")
                                 :object-type 'alist :array-type 'list)))
    (unless (eql (alist-get 'version dump) 1)
      (error "Unsupported hunkle protocol version; update hunkle.el"))
    (setq hunkle--files (alist-get 'files dump)
          hunkle--token (alist-get 'token dump)
          hunkle--branch (alist-get 'branch dump)
          hunkle--commits nil
          hunkle--assign (make-hash-table :test #'equal))
    (when (null hunkle--files)
      (user-error "No staged changes"))))

(defun hunkle--key-label (ci)
  "Key sequence addressing commit index CI: \"1\"-\"9\", then \"010\"..."
  (if (< ci 9)
      (number-to-string (1+ ci))
    (format "0%d" (1+ ci))))

(defun hunkle--hunk-lines (fi hi)
  "The line alists of hunk HI in file FI."
  (alist-get 'lines (nth hi (alist-get 'hunks (nth fi hunkle--files)))))

(defun hunkle--hunk-locs (fi hi)
  "Locs of all changed lines of hunk HI in file FI."
  (let ((locs nil) (li 0))
    (dolist (line (hunkle--hunk-lines fi hi))
      (unless (equal (alist-get 'kind line) "context")
        (push (list fi hi li) locs))
      (setq li (1+ li)))
    (nreverse locs)))

(defun hunkle--commit-stats (ci)
  "Return (ADDED . DELETED) line counts assigned to commit CI."
  (let ((add 0) (del 0))
    (maphash (lambda (loc c)
               (when (eql c ci)
                 (pcase-let ((`(,fi ,hi ,li) loc))
                   (pcase (alist-get 'kind (nth li (hunkle--hunk-lines fi hi)))
                     ("add" (setq add (1+ add)))
                     ("del" (setq del (1+ del)))))))
             hunkle--assign)
    (cons add del)))

(defun hunkle--unassigned-count ()
  "Number of changed lines not assigned to any commit."
  (let ((n 0) (fi 0))
    (dolist (file hunkle--files)
      (let ((hi 0))
        (dolist (_hunk (alist-get 'hunks file))
          (dolist (loc (hunkle--hunk-locs fi hi))
            (unless (gethash loc hunkle--assign)
              (setq n (1+ n))))
          (setq hi (1+ hi))))
      (setq fi (1+ fi)))
    n))

(defun hunkle--render ()
  "Redraw the buffer from the current state, keeping point on its line."
  (let ((inhibit-read-only t)
        (loc (get-text-property (pos-bol) 'hunkle-loc))
        (line (line-number-at-pos)))
    (erase-buffer)
    (magit-insert-section (magit-section 'hunkle-root)
      (hunkle--insert-commits)
      (insert "\n")
      (hunkle--insert-files))
    (goto-char (point-min))
    (if-let* ((pos (and loc (hunkle--find-loc loc))))
        (goto-char pos)
      (forward-line (1- line)))))

(defun hunkle--find-loc (loc)
  "Position of the line carrying the `hunkle-loc' LOC, if any."
  (let ((pos (point-min)) found)
    (while (and (not found) pos)
      (when (equal (get-text-property pos 'hunkle-loc) loc)
        (setq found pos))
      (setq pos (next-single-property-change pos 'hunkle-loc)))
    found))

(defun hunkle--insert-commits ()
  (magit-insert-section (magit-section 'hunkle-commits)
    (magit-insert-heading (format "Commits (%d)" (length hunkle--commits)))
    (if (null hunkle--commits)
        (insert (hunkle--face "  none yet -- press n on a hunk\n" 'shadow))
      (let ((ci 0))
        (dolist (msg hunkle--commits)
          (let ((stats (hunkle--commit-stats ci)))
            (insert
             (propertize
              (concat "  "
                      (hunkle--face (format "[%s]" (hunkle--key-label ci))
                                    'hunkle-key)
                      " " msg
                      (hunkle--face (format "  (+%d -%d)" (car stats) (cdr stats))
                                    'shadow)
                      "\n")
              'hunkle-commit ci)))
          (setq ci (1+ ci)))))))

(defun hunkle--insert-files ()
  (magit-insert-section (magit-section 'hunkle-staged)
    (magit-insert-heading
      (format "Staged changes (%d unassigned lines)" (hunkle--unassigned-count)))
    (let ((fi 0))
      (dolist (file hunkle--files)
        (hunkle--insert-file fi file)
        (setq fi (1+ fi))))))

(defun hunkle--insert-file (fi file)
  (let ((path (alist-get 'path file))
        (kind (alist-get 'kind file)))
    (magit-insert-section (magit-section (list 'hunkle-file (alist-get 'path file)))
      (magit-insert-heading
        (concat (propertize path 'font-lock-face 'magit-diff-file-heading)
                (and (not (equal kind "modified"))
                     (propertize (format " (%s)" kind) 'font-lock-face 'shadow))
                (and (eq (alist-get 'binary file) t)
                     (propertize " (binary -- left staged)"
                                 'font-lock-face 'shadow))))
      (let ((hi 0))
        (dolist (hunk (alist-get 'hunks file))
          (hunkle--insert-hunk fi hi hunk)
          (setq hi (1+ hi)))))))

(defun hunkle--insert-hunk (fi hi hunk)
  (magit-insert-section (magit-section (list 'hunkle-hunk fi hi))
    (magit-insert-heading
      (propertize (alist-get 'header hunk)
                  'font-lock-face 'magit-diff-hunk-heading))
    (let ((li 0))
      (dolist (line (alist-get 'lines hunk))
        (let* ((kind (alist-get 'kind line))
               (content (alist-get 'content line))
               (change (not (equal kind "context")))
               (loc (and change (list fi hi li)))
               (ci (and loc (gethash loc hunkle--assign)))
               (tag (if ci (format "[%s]" (hunkle--key-label ci)) ""))
               (gutter (format "%-6s" tag))
               (prefix (pcase kind ("add" "+") ("del" "-") (_ " ")))
               (face (pcase kind
                       ("add" 'magit-diff-added)
                       ("del" 'magit-diff-removed)
                       (_ 'magit-diff-context)))
               (beg (point)))
          (insert gutter prefix content "\n")
          (let ((end (point)))
            (hunkle--put-face beg end face)
            (when loc (put-text-property beg end 'hunkle-loc loc))
            (hunkle--put-face beg (+ beg (length gutter)) (list 'hunkle-tag face))))
        (setq li (1+ li))))))

(defun hunkle--region-locs ()
  "Locs of the changed lines covered by an active region, if any.
Independent of `transient-mark-mode' so the `v' selection always
works."
  (when (and mark-active (mark t))
    (let* ((beg (min (point) (mark t)))
           (end (max (point) (mark t)))
           (empty (= beg end))
           locs done)
      (save-excursion
        (goto-char beg)
        (forward-line 0)
        (while (and (not done)
                    (if empty (<= (point) end) (< (point) end)))
          (when-let* ((loc (get-text-property (point) 'hunkle-loc)))
            (push loc locs))
          (if (eobp) (setq done t) (forward-line 1))))
      (deactivate-mark)
      (delete-dups (nreverse locs)))))

(defun hunkle--hunk-at-point ()
  "The (FI HI) of the hunk section at point, if any."
  (let ((sec (magit-current-section)))
    (while (and sec
                (not (and (listp (oref sec value))
                          (eq (car-safe (oref sec value)) 'hunkle-hunk))))
      (setq sec (oref sec parent)))
    (when sec (cdr (oref sec value)))))

(defun hunkle--targets ()
  "Locs to act on: the region's changed lines, else the hunk at point."
  (or (hunkle--region-locs)
      (pcase (hunkle--hunk-at-point)
        (`(,fi ,hi) (hunkle--hunk-locs fi hi)))))

(defun hunkle-begin-selection ()
  "Start selecting lines for a partial-hunk assignment.
Sets the mark at point; move (e.g. with the arrow keys) to extend
the region, then press a commit key to assign just those lines.
This is the equivalent of the TUI's `v' line-picking."
  (interactive)
  (push-mark (point) t t)
  (message "Select lines (move to extend), then a commit key assigns just those"))

(defun hunkle-select-line ()
  "Select the whole current line, vi `V' style.
Grabs the entire line regardless of column and keeps point on it,
with the region active so you can extend by lines.  Press a commit
key to assign the selected lines."
  (interactive)
  (let ((bol (line-beginning-position)))
    (goto-char (line-end-position))
    (push-mark bol t t))
  (message "Line selection (move to extend), then a commit key assigns"))

(defun hunkle--assign-locs (locs ci)
  "Assign every loc in LOCS to commit CI and redraw."
  (dolist (loc locs)
    (puthash loc ci hunkle--assign))
  (hunkle--render)
  (message "%d line(s) -> [%s] %s"
           (length locs) (hunkle--key-label ci) (nth ci hunkle--commits)))

(defun hunkle--check-commit (ci)
  (unless (and (>= ci 0) (< ci (length hunkle--commits)))
    (user-error "No commit [%s] yet -- press n to create one"
                (hunkle--key-label ci))))

(defun hunkle-assign-digit ()
  "Assign the hunk at point (or region lines) to the commit of the key pressed."
  (interactive)
  (let ((ci (- last-command-event ?1)))
    (hunkle--check-commit ci)
    (hunkle--assign-locs
     (or (hunkle--targets) (user-error "No hunk at point")) ci)))

(defun hunkle-assign-number (n)
  "Assign the hunk at point (or region lines) to commit number N."
  (interactive (list (read-number "Commit number: ")))
  (hunkle--check-commit (1- n))
  (hunkle--assign-locs
   (or (hunkle--targets) (user-error "No hunk at point")) (1- n)))

(defun hunkle-new-commit (msg)
  "Create a new commit with message MSG; assign the hunk/region to it."
  (interactive (list (read-string "New commit message: ")))
  (when (string-empty-p (string-trim msg))
    (user-error "Commit message cannot be empty"))
  (let ((targets (hunkle--targets)))
    (setq hunkle--commits (append hunkle--commits (list msg)))
    (if targets
        (hunkle--assign-locs targets (1- (length hunkle--commits)))
      (hunkle--render)
      (message "Created [%s] %s -- nothing assigned yet"
               (hunkle--key-label (1- (length hunkle--commits))) msg))))

(defun hunkle-unassign ()
  "Unassign the hunk at point (or the region's lines)."
  (interactive)
  (let ((locs (or (hunkle--targets) (user-error "No hunk at point"))))
    (dolist (loc locs)
      (remhash loc hunkle--assign))
    (hunkle--render)
    (message "Unassigned %d line(s)" (length locs))))

(defun hunkle--commit-at-point-or-read (prompt)
  "Commit index at point, or read a commit number with PROMPT."
  (or (get-text-property (pos-bol) 'hunkle-commit)
      (let ((n (read-number prompt)))
        (hunkle--check-commit (1- n))
        (1- n))))

(defun hunkle-edit-message ()
  "Edit the message of the commit at point (or by number)."
  (interactive)
  (unless hunkle--commits (user-error "No commits yet"))
  (let* ((ci (hunkle--commit-at-point-or-read "Edit message of commit number: "))
         (msg (read-string (format "Message for [%s]: " (hunkle--key-label ci))
                           (nth ci hunkle--commits))))
    (when (string-empty-p (string-trim msg))
      (user-error "Commit message cannot be empty"))
    (setf (nth ci hunkle--commits) msg)
    (hunkle--render)))

(defun hunkle-swap-commits ()
  "Swap two commits' numbers (this also swaps their creation order)."
  (interactive)
  (unless (cdr hunkle--commits) (user-error "Need at least two commits"))
  (let* ((a (hunkle--commit-at-point-or-read "Swap commit number: "))
         (b (1- (read-number (format "Swap [%s] with commit number: "
                                     (hunkle--key-label a))))))
    (hunkle--check-commit b)
    (when (eql a b) (user-error "Cannot swap a commit with itself"))
    (cl-rotatef (nth a hunkle--commits) (nth b hunkle--commits))
    (maphash (lambda (loc ci)
               (cond ((eql ci a) (puthash loc b hunkle--assign))
                     ((eql ci b) (puthash loc a hunkle--assign))))
             hunkle--assign)
    (hunkle--render)
    (message "Swapped [%s] <-> [%s]"
             (hunkle--key-label a) (hunkle--key-label b))))

(defun hunkle-refresh ()
  "Reload the staged changes, discarding all assignments."
  (interactive)
  (when (or (hash-table-empty-p hunkle--assign)
            (yes-or-no-p "Discard assignments and reload? "))
    (hunkle--load)
    (hunkle--render)))

(defun hunkle--plan-json ()
  "Serialize the current assignments as a plan for `hunkle apply'."
  (let (entries)
    (maphash (lambda (loc ci)
               (push (vconcat (append loc (list ci))) entries))
             hunkle--assign)
    (json-serialize
     `((token . ,hunkle--token)
       (commits . ,(vconcat hunkle--commits))
       (assignments . ,(vconcat (nreverse entries)))))))

(defun hunkle--apply ()
  "Run `hunkle apply' with the current plan; return the created commits."
  (let ((result (json-parse-string (hunkle--call (hunkle--plan-json) "apply")
                                   :object-type 'alist :array-type 'list)))
    (alist-get 'commits result)))

(defun hunkle-create-commits ()
  "Create the planned commits."
  (interactive)
  (when (hash-table-empty-p hunkle--assign)
    (user-error "Nothing assigned yet"))
  (let ((n (length (delete-dups (hash-table-values hunkle--assign))))
        (left (hunkle--unassigned-count)))
    (when (y-or-n-p (format "Create %d commit(s) on %s%s? " n hunkle--branch
                            (if (> left 0)
                                (format " (%d lines stay staged)" left)
                              "")))
      (let ((commits (hunkle--apply)))
        (message "hunkle: created %d commit(s): %s"
                 (length commits)
                 (mapconcat (lambda (c)
                              (format "%.10s %s"
                                      (alist-get 'id c) (alist-get 'message c)))
                            commits ", "))
        (when (fboundp 'magit-refresh-all)
          (magit-refresh-all))
        (if (> left 0)
            (hunkle--soft-reload)
          (quit-window t))))))

(defun hunkle-quit ()
  "Quit the hunkle buffer without creating any commits.
Staged changes are left untouched.  Asks for confirmation if any
assignments would be discarded."
  (interactive)
  (when (or (null hunkle--assign)
            (hash-table-empty-p hunkle--assign)
            (yes-or-no-p "Discard assignments and quit? "))
    (quit-window t)))

(defun hunkle--soft-reload ()
  "Reload after committing; close the buffer if nothing is staged anymore."
  (condition-case nil
      (progn (hunkle--load) (hunkle--render))
    (user-error (quit-window t))))

(defvar hunkle-mode-map
  (let ((map (make-sparse-keymap)))
    (set-keymap-parent map magit-section-mode-map)
    (dolist (k '("1" "2" "3" "4" "5" "6" "7" "8" "9"))
      (define-key map (kbd k) #'hunkle-assign-digit))
    (define-key map (kbd "0") #'hunkle-assign-number)
    (define-key map (kbd "n") #'hunkle-new-commit)
    (define-key map (kbd "v") #'hunkle-begin-selection)
    (define-key map (kbd "V") #'hunkle-select-line)
    (define-key map (kbd "u") #'hunkle-unassign)
    (define-key map (kbd "e") #'hunkle-edit-message)
    (define-key map (kbd "m") #'hunkle-swap-commits)
    (define-key map (kbd "g") #'hunkle-refresh)
    (define-key map (kbd "C-c C-c") #'hunkle-create-commits)
    (define-key map (kbd "C-c C-k") #'hunkle-quit)
    map)
  "Keymap for `hunkle-mode'.")

(define-derived-mode hunkle-mode magit-section-mode "Hunkle"
  "Major mode for sorting staged hunks into commits."
  :group 'hunkle
  (setq-local magit-section-highlight-hook nil))

;;;###autoload
(defun hunkle ()
  "Split the staged changes of the current repository into commits."
  (interactive)
  (let* ((dir (or (locate-dominating-file default-directory ".git")
                  (user-error "Not inside a git repository")))
         (buf (get-buffer-create
               (format "*hunkle: %s*"
                       (file-name-nondirectory (directory-file-name dir))))))
    (with-current-buffer buf
      (hunkle-mode)
      (setq default-directory dir)
      (hunkle--load)
      (let ((inhibit-read-only t)) (erase-buffer))
      (hunkle--render)
      (goto-char (point-min)))
    (switch-to-buffer buf)))

;;;###autoload
(defun hunkle-magit-setup ()
  "Integrate hunkle with Magit.
Bind `hunkle-magit-key' to `hunkle' in the Magit status buffer and
add an entry for it to the `magit-dispatch' menu (the `?' popup)."
  (with-eval-after-load 'magit-status
    (define-key (symbol-value 'magit-status-mode-map)
                (kbd hunkle-magit-key) #'hunkle))
  (with-eval-after-load 'magit
    (unless (ignore-errors (transient-get-suffix 'magit-dispatch hunkle-magit-key))
      (transient-append-suffix 'magit-dispatch "!"
        (list hunkle-magit-key "Split staged into commits (hunkle)" #'hunkle)))))

(provide 'hunkle)
;;; hunkle.el ends here
