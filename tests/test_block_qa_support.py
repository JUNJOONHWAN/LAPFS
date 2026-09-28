import sys,unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))
from block_qa_support import find_apfs_partition
class PartitionTests(unittest.TestCase):
 def setUp(self):
  self.disk={'path':'/dev/loop29','type':'loop'}
  self.part={'path':'/dev/loop29p1','type':'part','fstype':'apfs','partuuid':'uuid'}
 def test_flat(self):self.assertEqual(find_apfs_partition({'blockdevices':[self.disk,self.part]},'/dev/loop29'),self.part)
 def test_tree(self):self.assertEqual(find_apfs_partition({'blockdevices':[dict(self.disk,children=[self.part])]},'/dev/loop29'),self.part)
 def test_mixed_duplicate(self):self.assertEqual(find_apfs_partition({'blockdevices':[dict(self.disk,children=[self.part]),self.part]},'/dev/loop29'),self.part)
 def test_never_select_physical_or_other_loop(self):
  for p in ['/dev/sda2','/dev/loop290p1','/dev/loop29p1evil']:
   with self.assertRaises(RuntimeError):find_apfs_partition({'blockdevices':[dict(self.part,path=p)]},'/dev/loop29')
 def test_ambiguous_and_missing(self):
  for rows in [[],[self.part,dict(self.part,path='/dev/loop29p2')]]:
   with self.assertRaises(RuntimeError):find_apfs_partition({'blockdevices':rows},'/dev/loop29')
 def test_reject_nonloop_input(self):
  with self.assertRaises(ValueError):find_apfs_partition({'blockdevices':[self.part]},'/dev/sda')
if __name__=='__main__':unittest.main()
