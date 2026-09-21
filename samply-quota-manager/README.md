# samply-quota-manager

Limits the total size of a directory by deleting least-recently-used files.

`QuotaManager` keeps an inventory of the files in a managed directory in a
sqlite database. You tell it about created and accessed files, and it enforces
three optional limits when asked to evict:

 - `set_max_age`: delete files which haven't been accessed for this long.
 - `set_max_total_size`: delete least-recently-used files until the total size
   is below the limit.
 - `set_min_age`: never delete files which were accessed within this time span,
   even if that means the size limit is exceeded.

