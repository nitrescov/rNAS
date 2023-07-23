# This file creates hashed values from the user credentials and can be used to set up the necessary user directory.
# Copyright (C) 2023  Nico Pieplow (nitrescov)
# Contact: nitrescov@protonmail.com
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published
# by the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.
#
# You should have received a copy of the GNU Affero General Public License
# along with this program.  If not, see <https://www.gnu.org/licenses/>.

import hashlib
import os
import sys

if not os.path.isfile("users.csv"):
    print("The file users.csv does not exist. Should it be created now? (y/n)")
    if input() == "y":
        userfile = open("users.csv", "w", encoding="utf-8")
        userfile.close()
    else:
        sys.exit()

while True:
    print("\n--- Add user ---\n")
    name = input("name: ")
    password = input("password: ")
    hashed = hashlib.sha384(str(password).encode("utf-8") + str(name).encode("utf-8")).hexdigest()
    with open("users.csv", "a", encoding="utf-8") as userfile:
        userfile.write(hashed + ";" + str(name) + "\n")
    print("\nAdd another user? (y/n)")
    if input() != "y":
        break

print("\n\nDo you wish to create the required directories now? (y/n)")
if input() == "y":
    with open("users.csv", "r", encoding="utf-8") as userfile:
        for line in userfile.readlines():
            current_user = line.split(";")[1].replace("\n", "")
            if not os.path.isdir(current_user):
                os.mkdir(current_user)
    print("\n\nTo save the uploaded files in another directory, move the user and tmp folders to the respective target directory.")

print("\n\nDone.")
